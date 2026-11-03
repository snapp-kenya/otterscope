package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/otterscope/otterscope/internal/model"
)

// UpsertSteps writes a batch of steps and re-derives the aggregates of every
// touched run, all in one transaction. Steps are keyed by (project, span ID),
// so re-delivered OTLP batches are naturally idempotent.
func (s *Store) UpsertSteps(ctx context.Context, steps []model.Step) error {
	_, err := s.upsertSteps(ctx, steps)
	return err
}

// upsertSteps is UpsertSteps plus the count of steps refused because the row
// already stored under that (project, span ID) belongs to another service.
func (s *Store) upsertSteps(ctx context.Context, steps []model.Step) (int, error) {
	if len(steps) == 0 {
		return 0, nil
	}
	tx, err := s.writer.BeginTx(ctx, nil)
	if err != nil {
		return 0, err
	}
	defer tx.Rollback()
	rejected, err := upsertStepsTx(ctx, tx, steps)
	if err != nil {
		return 0, err
	}
	return rejected, tx.Commit()
}

// upsertStepsTx is the transactional body of UpsertSteps, shared with
// IngestBatch. It returns the number of steps it refused to write.
//
// Overwriting an existing (project, span ID) is only allowed when the incoming
// step names exactly the same resource service as the stored one: a span ID is
// not a capability, so an OTLP sender must not be able to rewrite a step that
// some other service emitted — including its kind, name and message text,
// which downstream agents read. A mismatch writes nothing at all (no fallback
// to the stored row's fields) and is counted as rejected.
func upsertStepsTx(ctx context.Context, tx *sql.Tx, steps []model.Step) (int, error) {
	if len(steps) == 0 {
		return 0, nil
	}
	ins, err := tx.PrepareContext(ctx, `
		INSERT INTO steps
		(id, run_id, parent_id, kind, name, project, service, agent_name, status,
		 start_ns, end_ns, error,
		 provider, request_model, response_model, input_tokens, output_tokens,
		 cache_read_tokens, cache_creation_tokens, reasoning_tokens,
		 tool_name, tool_call_id, detail, cost_usd, prompt)
		VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
		ON CONFLICT(project, id) DO UPDATE SET
		 run_id=excluded.run_id, parent_id=excluded.parent_id,
		 kind=excluded.kind, name=excluded.name, service=excluded.service,
		 agent_name=excluded.agent_name, status=excluded.status,
		 start_ns=excluded.start_ns, end_ns=excluded.end_ns, error=excluded.error,
		 provider=excluded.provider, request_model=excluded.request_model,
		 response_model=excluded.response_model, input_tokens=excluded.input_tokens,
		 output_tokens=excluded.output_tokens,
		 cache_read_tokens=excluded.cache_read_tokens,
		 cache_creation_tokens=excluded.cache_creation_tokens,
		 reasoning_tokens=excluded.reasoning_tokens,
		 tool_name=excluded.tool_name, tool_call_id=excluded.tool_call_id,
		 detail=excluded.detail, cost_usd=excluded.cost_usd, prompt=excluded.prompt
		WHERE steps.service = excluded.service`)
	if err != nil {
		return 0, err
	}
	defer ins.Close()

	// FTS index is kept in sync per step: delete any prior row (idempotent
	// re-delivery) then insert the searchable content.
	ftsDel, err := tx.PrepareContext(ctx, `DELETE FROM steps_fts WHERE project = ? AND step_id = ?`)
	if err != nil {
		return 0, err
	}
	defer ftsDel.Close()
	ftsIns, err := tx.PrepareContext(ctx,
		`INSERT INTO steps_fts (project, run_id, step_id, content) VALUES (?,?,?,?)`)
	if err != nil {
		return 0, err
	}
	defer ftsIns.Close()

	// Runs are keyed by (project, run_id); collect the distinct pairs a write
	// actually touched and re-derive them together in rederiveRuns.
	var runKeys []runKey
	seen := make(map[runKey]bool)
	rejected := 0
	for _, st := range steps {
		if st.Project == "" {
			st.Project = "default"
		}
		var llm model.LLMCall
		if st.LLM != nil {
			llm = *st.LLM
		}
		var tool model.ToolCall
		if st.Tool != nil {
			tool = *st.Tool
		}
		res, err := ins.ExecContext(ctx,
			st.ID, st.RunID, st.ParentID, string(st.Kind), st.Name, st.Project, st.Service, st.AgentName, string(st.Status),
			st.Start.UnixNano(), st.End.UnixNano(), st.Error,
			llm.Provider, llm.RequestModel, llm.ResponseModel, llm.InputTokens, llm.OutputTokens,
			llm.CacheReadTokens, llm.CacheCreationTokens, llm.ReasoningTokens,
			tool.Name, tool.CallID, marshalDetail(st), nullableFloat(llm.CostUSD), llm.Prompt,
		)
		if err != nil {
			return 0, fmt.Errorf("insert step %s: %w", st.ID, err)
		}
		// The guarded upsert writes nothing when the stored row belongs to
		// another service; sqlite reports that as zero affected rows.
		if n, err := res.RowsAffected(); err != nil {
			return 0, fmt.Errorf("insert step %s: %w", st.ID, err)
		} else if n == 0 {
			rejected++
			continue
		}
		if _, err := ftsDel.ExecContext(ctx, st.Project, st.ID); err != nil {
			return 0, fmt.Errorf("fts delete %s: %w", st.ID, err)
		}
		if _, err := ftsIns.ExecContext(ctx, st.Project, st.RunID, st.ID, ftsContent(st)); err != nil {
			return 0, fmt.Errorf("fts insert %s: %w", st.ID, err)
		}
		k := runKey{st.Project, st.RunID}
		if !seen[k] {
			seen[k] = true
			runKeys = append(runKeys, k)
		}
	}

	if err := rederiveRuns(ctx, tx, runKeys); err != nil {
		return 0, err
	}
	return rejected, nil
}

// runKey identifies a run within a project.
type runKey struct{ project, runID string }

// rederiveRuns recomputes run rows entirely from their steps, making run
// aggregates structurally idempotent instead of bookkept. Scoping by project
// prevents one project's steps from being merged into another's run (#49).
//
// Every touched run is re-derived by ONE grouped query rather than one
// aggregate query per run: a batch of N distinct trace IDs used to cost N*7
// statements, which let a single request monopolise the sole SQLite writer.
func rederiveRuns(ctx context.Context, tx *sql.Tx, keys []runKey) error {
	if len(keys) == 0 {
		return nil
	}
	// A temp table keeps the GROUP BY bounded by the touched runs without
	// splatting thousands of parameters into the statement.
	if _, err := tx.ExecContext(ctx, `
		CREATE TEMP TABLE IF NOT EXISTS touched_runs (
			project TEXT NOT NULL, run_id TEXT NOT NULL, PRIMARY KEY (project, run_id))`); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM touched_runs`); err != nil {
		return err
	}
	touch, err := tx.PrepareContext(ctx, `INSERT OR IGNORE INTO touched_runs (project, run_id) VALUES (?, ?)`)
	if err != nil {
		return err
	}
	for _, k := range keys {
		if _, err := touch.ExecContext(ctx, k.project, k.runID); err != nil {
			touch.Close()
			return err
		}
	}
	touch.Close()

	type derivedRun struct {
		project, runID                           string
		service, agent, errText, models, prompts string
		startNS, endNS                           int64
		inTok, outTok, llmCalls, toolCalls       int64
		hasError, hasRoot, partial               int64
		cost                                     sql.NullFloat64
	}
	// Rows are drained before any write: the writer is a single connection, so
	// a cursor held open across an Exec would wait on itself.
	var derived []derivedRun
	rows, err := tx.QueryContext(ctx, `
		SELECT t.project, t.run_id,
		       min(s.start_ns), max(s.end_ns),
		       sum(s.input_tokens), sum(s.output_tokens),
		       sum(s.kind = 'llm'), sum(s.kind = 'tool'),
		       max(s.error != ''), max(s.parent_id = ''),
		       coalesce((SELECT x.service FROM steps x
		                  WHERE x.project = t.project AND x.run_id = t.run_id AND x.service != ''
		                  ORDER BY x.start_ns LIMIT 1), ''),
		       coalesce((SELECT x.agent_name FROM steps x
		                  WHERE x.project = t.project AND x.run_id = t.run_id AND x.agent_name != ''
		                  ORDER BY x.start_ns LIMIT 1), ''),
		       coalesce((SELECT x.error FROM steps x
		                  WHERE x.project = t.project AND x.run_id = t.run_id AND x.error != ''
		                  ORDER BY x.start_ns LIMIT 1), ''),
		       coalesce(group_concat(DISTINCT CASE WHEN s.request_model != '' THEN s.request_model END), ''),
		       coalesce(group_concat(DISTINCT CASE WHEN s.prompt != '' THEN s.prompt END), ''),
		       sum(s.cost_usd),
		       max(s.kind = 'llm' AND s.cost_usd IS NULL AND (s.input_tokens > 0 OR s.output_tokens > 0))
		FROM touched_runs t
		JOIN steps s ON s.project = t.project AND s.run_id = t.run_id
		GROUP BY t.project, t.run_id`)
	if err != nil {
		return err
	}
	for rows.Next() {
		var d derivedRun
		if err := rows.Scan(&d.project, &d.runID, &d.startNS, &d.endNS,
			&d.inTok, &d.outTok, &d.llmCalls, &d.toolCalls,
			&d.hasError, &d.hasRoot,
			&d.service, &d.agent, &d.errText, &d.models, &d.prompts,
			&d.cost, &d.partial); err != nil {
			rows.Close()
			return err
		}
		derived = append(derived, d)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return err
	}
	rows.Close()

	upsert, err := tx.PrepareContext(ctx, `
		INSERT INTO runs (project, id, service, agent_name, status, start_ns, end_ns,
		                  input_tokens, output_tokens, llm_calls, tool_calls, models, prompts,
		                  cost_usd, cost_partial, error)
		VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
		ON CONFLICT(project, id) DO UPDATE SET
		  service=excluded.service, agent_name=excluded.agent_name,
		  status=excluded.status, start_ns=excluded.start_ns, end_ns=excluded.end_ns,
		  input_tokens=excluded.input_tokens, output_tokens=excluded.output_tokens,
		  llm_calls=excluded.llm_calls, tool_calls=excluded.tool_calls,
		  models=excluded.models, prompts=excluded.prompts, cost_usd=excluded.cost_usd,
		  cost_partial=excluded.cost_partial, error=excluded.error`)
	if err != nil {
		return err
	}
	defer upsert.Close()
	for _, d := range derived {
		status := model.StatusRunning
		switch {
		case d.hasError != 0:
			status = model.StatusError
		case d.hasRoot != 0:
			status = model.StatusOK
		}
		if _, err := upsert.ExecContext(ctx,
			d.project, d.runID, d.service, d.agent, string(status), d.startNS, d.endNS,
			d.inTok, d.outTok, d.llmCalls, d.toolCalls, d.models, d.prompts,
			d.cost, d.partial != 0, d.errText); err != nil {
			return fmt.Errorf("rederive run %s/%s: %w", d.project, d.runID, err)
		}
	}
	return nil
}

// nullableFloat converts *float64 to a driver-friendly NULL-able value.
func nullableFloat(f *float64) any {
	if f == nil {
		return nil
	}
	return *f
}

func floatPtr(f sql.NullFloat64) *float64 {
	if !f.Valid {
		return nil
	}
	return &f.Float64
}

// Filter narrows ListRuns. Zero values mean "no constraint".
type Filter struct {
	Project string    // exact match
	Status  string    // exact match: running | ok | error
	Service string    // prefix match (index-friendly)
	Model   string    // substring match against the run's models list
	Prompt  string    // substring match against the run's prompt identities
	Query   string    // full-text search over step content (messages, tool i/o)
	Since   time.Time // start >= Since
	Until   time.Time // start <= Until
}

// ListRuns returns runs newest-first. offset-based paging is fine at target
// scale; revisit with keyset paging if it ever shows up in profiles.
func (s *Store) ListRuns(ctx context.Context, f Filter, limit, offset int) ([]model.Run, error) {
	where, args := filterWhere(f)
	args = append(args, limit, offset)

	rows, err := s.reader.QueryContext(ctx, `
		SELECT id, project, service, agent_name, status, start_ns, end_ns,
		       input_tokens, output_tokens, llm_calls, tool_calls, models, prompts,
		       cost_usd, cost_partial, error
		FROM runs`+where+` ORDER BY start_ns DESC LIMIT ? OFFSET ?`, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var runs []model.Run
	for rows.Next() {
		var r model.Run
		var status string
		var startNS, endNS int64
		var cost sql.NullFloat64
		if err := rows.Scan(&r.ID, &r.Project, &r.Service, &r.AgentName, &status, &startNS, &endNS,
			&r.InputTokens, &r.OutputTokens, &r.LLMCalls, &r.ToolCalls, &r.Models, &r.Prompts,
			&cost, &r.CostPartial, &r.Error); err != nil {
			return nil, err
		}
		r.CostUSD = floatPtr(cost)
		r.Status = model.Status(status)
		r.Start = time.Unix(0, startNS)
		r.End = time.Unix(0, endNS)
		runs = append(runs, r)
	}
	return runs, rows.Err()
}

// CountRuns returns how many runs match f, ignoring paging. Callers that cap
// their work use it to report exactly how much a cap left undone — "at least
// one left over" is not a number anyone can act on.
func (s *Store) CountRuns(ctx context.Context, f Filter) (int, error) {
	where, args := filterWhere(f)
	var n int
	err := s.reader.QueryRowContext(ctx,
		`SELECT count(*) FROM runs`+where, args...).Scan(&n)
	return n, err
}

// escapeLike neutralizes LIKE wildcards in user input.
func escapeLike(s string) string {
	r := strings.NewReplacer(`\`, `\\`, `%`, `\%`, `_`, `\_`)
	return r.Replace(s)
}

// ftsContent builds the searchable text for a step: name, model/provider,
// message contents, and tool i/o.
func ftsContent(st model.Step) string {
	var b strings.Builder
	b.WriteString(st.Name)
	if st.LLM != nil {
		b.WriteByte(' ')
		b.WriteString(st.LLM.Provider)
		b.WriteByte(' ')
		b.WriteString(st.LLM.RequestModel)
		for _, m := range st.LLM.InputMessages {
			b.WriteByte(' ')
			b.WriteString(m.Content)
		}
		for _, m := range st.LLM.OutputMessages {
			b.WriteByte(' ')
			b.WriteString(m.Content)
		}
	}
	if st.Tool != nil {
		b.WriteByte(' ')
		b.WriteString(st.Tool.Name)
		b.WriteByte(' ')
		b.WriteString(st.Tool.Arguments)
		b.WriteByte(' ')
		b.WriteString(st.Tool.Result)
	}
	return b.String()
}

// ftsQuery turns free-text into a safe FTS5 MATCH expression: each
// whitespace term becomes a quoted token (neutralizing FTS operators and
// punctuation), AND-ed together. Returns "" when there's nothing to match.
func ftsQuery(q string) string {
	fields := strings.Fields(q)
	if len(fields) == 0 {
		return ""
	}
	for i, f := range fields {
		fields[i] = `"` + strings.ReplaceAll(f, `"`, `""`) + `"`
	}
	return strings.Join(fields, " ")
}

// ErrNotFound means the requested row does not exist. It is a distinct
// sentinel (not an alias for sql.ErrNoRows) so callers must use errors.Is and
// so wrapping an error in the store cannot silently turn a 404 into a 500.
var ErrNotFound = errors.New("not found")

// ErrConflict means the write lost to a uniqueness constraint — a duplicate
// name, not an internal failure. Callers map it to 409; everything else is a
// 500.
var ErrConflict = errors.New("conflict")

// classifyWrite maps a SQLite write error to ErrConflict when it is a
// uniqueness violation, so the SQLite-specific detection stays in this
// package. Any other error is returned unchanged.
func classifyWrite(err error) error {
	if err == nil {
		return nil
	}
	if strings.Contains(err.Error(), "UNIQUE constraint failed") {
		return fmt.Errorf("%w: %s", ErrConflict, err)
	}
	return err
}

// deleted reports ErrNotFound when a DELETE matched no rows, so callers can
// tell "revoked it" from "there was nothing to revoke".
func deleted(res sql.Result) error {
	n, err := res.RowsAffected()
	if err != nil {
		return err
	}
	if n == 0 {
		return ErrNotFound
	}
	return nil
}

// GetRun returns a run and its steps ordered by start time. Trace ID can
// exist in more than one project (forging can't overwrite, only coexist);
// newest wins for this bare-ID lookup.
func (s *Store) GetRun(ctx context.Context, id string) (model.Run, []model.Step, error) {
	return s.getRun(ctx, "id = ? ORDER BY start_ns DESC LIMIT 1", id)
}

// GetRunInProject returns a run scoped to an exact project, so a shared token
// can't surface another project's run that happens to share a trace id (#49).
func (s *Store) GetRunInProject(ctx context.Context, project, id string) (model.Run, []model.Step, error) {
	return s.getRun(ctx, "project = ? AND id = ?", project, id)
}

func (s *Store) getRun(ctx context.Context, where string, args ...any) (model.Run, []model.Step, error) {
	var r model.Run
	var status string
	var startNS, endNS int64
	var cost sql.NullFloat64
	err := s.reader.QueryRowContext(ctx, `
		SELECT id, project, service, agent_name, status, start_ns, end_ns,
		       input_tokens, output_tokens, llm_calls, tool_calls, models, prompts,
		       cost_usd, cost_partial, error
		FROM runs WHERE `+where, args...).
		Scan(&r.ID, &r.Project, &r.Service, &r.AgentName, &status, &startNS, &endNS,
			&r.InputTokens, &r.OutputTokens, &r.LLMCalls, &r.ToolCalls, &r.Models, &r.Prompts,
			&cost, &r.CostPartial, &r.Error)
	if errors.Is(err, sql.ErrNoRows) {
		return model.Run{}, nil, ErrNotFound
	}
	if err != nil {
		return model.Run{}, nil, err
	}
	r.CostUSD = floatPtr(cost)
	r.Status = model.Status(status)
	r.Start = time.Unix(0, startNS)
	r.End = time.Unix(0, endNS)

	steps, err := s.loadSteps(ctx, r.Project, r.ID)
	if err != nil {
		return model.Run{}, nil, err
	}
	return r, steps, nil
}

func (s *Store) loadSteps(ctx context.Context, project, runID string) ([]model.Step, error) {
	rows, err := s.reader.QueryContext(ctx, `
		SELECT id, run_id, parent_id, kind, name, service, agent_name, status,
		       start_ns, end_ns, error,
		       provider, request_model, response_model, input_tokens, output_tokens,
		       cache_read_tokens, cache_creation_tokens, reasoning_tokens,
		       tool_name, tool_call_id, detail, cost_usd, prompt
		FROM steps WHERE project = ? AND run_id = ? ORDER BY start_ns`, project, runID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var steps []model.Step
	for rows.Next() {
		var st model.Step
		var kind, stStatus string
		var sNS, eNS int64
		var llm model.LLMCall
		var tool model.ToolCall
		var detail, prompt string
		var stepCost sql.NullFloat64
		if err := rows.Scan(&st.ID, &st.RunID, &st.ParentID, &kind, &st.Name, &st.Service, &st.AgentName, &stStatus,
			&sNS, &eNS, &st.Error,
			&llm.Provider, &llm.RequestModel, &llm.ResponseModel, &llm.InputTokens, &llm.OutputTokens,
			&llm.CacheReadTokens, &llm.CacheCreationTokens, &llm.ReasoningTokens,
			&tool.Name, &tool.CallID, &detail, &stepCost, &prompt); err != nil {
			return nil, err
		}
		llm.CostUSD = floatPtr(stepCost)
		llm.Prompt = prompt
		st.Kind = model.StepKind(kind)
		st.Status = model.Status(stStatus)
		st.Start = time.Unix(0, sNS)
		st.End = time.Unix(0, eNS)
		if st.Kind == model.StepLLM {
			st.LLM = &llm
		}
		if st.Kind == model.StepTool {
			st.Tool = &tool
		}
		applyDetail(&st, detail)
		steps = append(steps, st)
	}
	return steps, rows.Err()
}
