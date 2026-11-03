package ingest

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/rand"
	"time"

	"github.com/otterscope/otterscope/internal/evals"
	"github.com/otterscope/otterscope/internal/model"
	"github.com/otterscope/otterscope/internal/store"
)

// Per-call caps on on-demand evaluation. Without them one request scored
// 40,003 runs, and with a judge key configured it made one paid model call per
// run against a model the assertion named — a cost bomb in a single POST.
// Both caps are vars, not consts, so tests can lower them without seeding
// thousands of rows (same pattern as csvExportLimit).
var (
	// MaxEvaluateRuns caps how many runs one evaluate call scores. Default
	// 500: enough to backfill a day of agent traffic, small enough that one
	// request cannot pin a core for minutes.
	MaxEvaluateRuns = 500
	// MaxEvaluateJudgeCalls caps paid judge calls in one evaluate call.
	// Default 100: the expensive half of the work, and the part that bills.
	MaxEvaluateJudgeCalls = 100
)

// ErrUnboundedEvaluate is returned when a call names neither a run, nor a
// time window, nor a limit. It is a sentinel so the HTTP layer can answer 400
// with a problem document instead of treating it as a server fault.
var ErrUnboundedEvaluate = errors.New(
	"unbounded evaluate: pass run=<id>, a since/until window, or limit=<n>")

// RunRef identifies a run for evaluation. A trace id alone does not: the
// same id can exist in more than one project, and resolving it without the
// project scored runs against the wrong project's assertions (#98).
type RunRef struct {
	Project string
	ID      string
}

// EvaluateOptions is one on-demand evaluation request. Exactly one bound
// kind is required: RunID, a Since/Until window, or Limit. All three may be
// combined (e.g. a window plus a limit), but none of them means the request
// is unbounded and Validate refuses it.
type EvaluateOptions struct {
	Project string
	RunID   string
	Since   time.Time
	Until   time.Time
	Limit   int // 0 = caller did not set one
}

// Validate enforces the "no unbounded evaluate" rule.
func (o EvaluateOptions) Validate() error {
	if o.RunID != "" {
		return nil
	}
	if !o.Since.IsZero() || !o.Until.IsZero() {
		if !o.Since.IsZero() && !o.Until.IsZero() && o.Until.Before(o.Since) {
			return fmt.Errorf("until (%s) is before since (%s)",
				o.Until.Format(time.RFC3339), o.Since.Format(time.RFC3339))
		}
		return nil
	}
	if o.Limit > 0 {
		return nil
	}
	return ErrUnboundedEvaluate
}

// EvaluateReport is what one call actually did, including the work the caps
// left undone. Skips are reported rather than hidden: a silently truncated
// backfill looks like a complete one and every rate computed from it is
// quietly wrong.
type EvaluateReport struct {
	RunsEvaluated     int `json:"runsEvaluated"`
	RunsSkipped       int `json:"runsSkipped"`
	RunLimit          int `json:"runLimit"`
	JudgeCalls        int `json:"judgeCalls"`
	JudgeCallsSkipped int `json:"judgeCallsSkipped"`
	JudgeCallLimit    int `json:"judgeCallLimit"`
}

// evalStats is what evaluateRuns did: runs scored, and judge calls made or
// skipped because of the budget.
type evalStats struct {
	runs         int
	judgeCalls   int
	judgeSkipped int
}

// judgeBudget caps paid judge calls in one pass. remaining < 0 means
// unlimited (the ingest path, already bounded by batch size).
type judgeBudget struct {
	remaining int
}

// allow spends one call from the budget, reporting false when it is spent
// out. A refused call is not handed to the judge at all.
func (b *judgeBudget) allow() bool {
	if b.remaining == 0 {
		return false
	}
	if b.remaining > 0 {
		b.remaining--
	}
	return true
}

// EvaluateRuns scores each completed run against its project's enabled
// assertions. Deterministic assertions always run; llm_judge assertions run
// subject to their sampleRate, or unconditionally when judgeAll is set
// (on-demand backfill). Safe to re-run: results upsert by
// (project, run, assertion).
//
// No judge budget: callers that need one use evaluateRuns directly (the
// on-demand path), because an ingest batch is already bounded and dropping
// its judge calls silently would leave runs permanently unscored.
func EvaluateRuns(ctx context.Context, st *store.Store, judge evals.Endpoint, refs []RunRef, judgeAll bool) error {
	_, err := evaluateRuns(ctx, st, judge, refs, judgeAll, &judgeBudget{remaining: -1})
	return err
}

func evaluateRuns(ctx context.Context, st *store.Store, judge evals.Endpoint, refs []RunRef, judgeAll bool, budget *judgeBudget) (evalStats, error) {
	var stats evalStats
	cache := map[string][]evals.Assertion{} // project → enabled assertions
	for _, ref := range refs {
		project := ref.Project
		if project == "" {
			project = "default"
		}
		run, steps, err := st.GetRunInProject(ctx, project, ref.ID)
		if err != nil {
			return stats, fmt.Errorf("load run %s/%s: %w", project, ref.ID, err)
		}
		if run.Status == model.StatusRunning {
			continue // scored once the root span arrives
		}
		stats.runs++
		asserts, ok := cache[run.Project]
		if !ok {
			all, err := st.ListAssertions(ctx, run.Project)
			if err != nil {
				return stats, err
			}
			for _, a := range all {
				if a.Enabled {
					asserts = append(asserts, a)
				}
			}
			cache[run.Project] = asserts
		}
		if len(asserts) == 0 {
			continue
		}
		// Existing results let us skip re-judging (a paid call) when a
		// batch for an already-scored run is redelivered.
		var judged map[int64]bool
		if !judgeAll {
			judged = map[int64]bool{}
			if existing, err := st.ResultsForRun(ctx, run.Project, run.ID); err == nil {
				for _, r := range existing {
					judged[r.AssertionID] = true
				}
			}
		}
		var results []evals.Result
		for _, a := range asserts {
			if a.Type == "llm_judge" {
				if judged[a.ID] {
					continue // already scored; don't re-pay on redelivery
				}
				if !judgeAll && !sampled(a) {
					continue
				}
				// Only a call that can actually reach the paid endpoint
				// spends the budget: no key or a model outside the
				// allow-list means Judge refuses without a request, and
				// charging for that would make real calls skip for nothing.
				if judge.Key != "" && judgeModelAllowed(judge, a) {
					if !budget.allow() {
						stats.judgeSkipped++
						continue
					}
					stats.judgeCalls++
				}
				results = append(results, evals.Judge(ctx, judge, a, run, steps))
				continue
			}
			results = append(results, evals.Evaluate(a, run, steps))
		}
		if err := st.SaveAssertionResults(ctx, run.Project, run.ID, results); err != nil {
			return stats, fmt.Errorf("save results for %s/%s: %w", run.Project, run.ID, err)
		}
	}
	return stats, nil
}

// judgeModelAllowed reports whether the assertion's model is in the
// allow-list. A malformed config is not allowed here; Judge reports it as a
// failed result instead.
func judgeModelAllowed(judge evals.Endpoint, a evals.Assertion) bool {
	model, err := a.JudgeModel()
	if err != nil {
		return false
	}
	return judge.AllowsModel(model)
}

// sampled applies the judge's sampleRate (0 or unset = judge everything).
func sampled(a evals.Assertion) bool {
	var cfg struct {
		SampleRate float64 `json:"sampleRate"`
	}
	if err := json.Unmarshal([]byte(a.Config), &cfg); err != nil {
		return false
	}
	if cfg.SampleRate <= 0 || cfg.SampleRate >= 1 {
		return true
	}
	return rand.Float64() < cfg.SampleRate
}

// EvaluateProject backfills assertion results over the runs an
// EvaluateOptions selects (on-demand evaluation). The call is bounded in
// three ways: the caller must name a run, a window, or a limit; at most
// MaxEvaluateRuns runs are scored; and at most MaxEvaluateJudgeCalls paid
// judge calls are made. What the caps skipped is reported, never dropped.
func EvaluateProject(ctx context.Context, st *store.Store, judge evals.Endpoint, opts EvaluateOptions) (EvaluateReport, error) {
	judgeCap := MaxEvaluateJudgeCalls
	if judgeCap < 0 {
		judgeCap = 0 // a negative cap is not "unlimited" on an endpoint
	}
	rep := EvaluateReport{
		RunLimit:       runCap(opts),
		JudgeCallLimit: judgeCap,
	}
	if err := opts.Validate(); err != nil {
		return rep, err
	}
	budget := &judgeBudget{remaining: rep.JudgeCallLimit}

	if opts.RunID != "" {
		project := opts.Project
		if project == "" {
			project = "default"
		}
		stats, err := evaluateRuns(ctx, st, judge,
			[]RunRef{{Project: project, ID: opts.RunID}}, true, budget)
		rep.RunsEvaluated = stats.runs
		rep.JudgeCalls = stats.judgeCalls
		rep.JudgeCallsSkipped = stats.judgeSkipped
		return rep, err
	}

	filter := store.Filter{Project: opts.Project, Since: opts.Since, Until: opts.Until}
	candidates, err := completedRunCount(ctx, st, filter)
	if err != nil {
		return rep, err
	}
	refs, err := collectRunRefs(ctx, st, filter, rep.RunLimit)
	if err != nil {
		return rep, err
	}
	stats, err := evaluateRuns(ctx, st, judge, refs, true, budget)
	rep.RunsEvaluated = stats.runs
	rep.JudgeCalls = stats.judgeCalls
	rep.JudgeCallsSkipped = stats.judgeSkipped
	if skipped := candidates - len(refs); skipped > 0 && err == nil {
		rep.RunsSkipped = skipped
	}
	return rep, err
}

// runCap is the effective per-call run cap: the server maximum, lowered by
// an explicit ?limit= the caller asked for.
func runCap(opts EvaluateOptions) int {
	limit := MaxEvaluateRuns
	if limit <= 0 {
		limit = 1
	}
	if opts.Limit > 0 && opts.Limit < limit {
		limit = opts.Limit
	}
	return limit
}

// completedRunCount counts the runs a window selects that are not still
// running, so the caller can be told exactly how many the cap left behind
// rather than "at least one".
func completedRunCount(ctx context.Context, st *store.Store, filter store.Filter) (int, error) {
	total, err := st.CountRuns(ctx, filter)
	if err != nil {
		return 0, err
	}
	running, err := st.CountRuns(ctx, store.Filter{
		Project: filter.Project, Since: filter.Since, Until: filter.Until,
		Status: string(model.StatusRunning),
	})
	if err != nil {
		return 0, err
	}
	if running > total {
		running = total
	}
	return total - running, nil
}

// collectRunRefs loads at most limit completed runs, paging so a wide window
// costs a bounded read instead of loading the whole table.
func collectRunRefs(ctx context.Context, st *store.Store, filter store.Filter, limit int) ([]RunRef, error) {
	const page = 200
	var refs []RunRef
	for offset := 0; len(refs) < limit; offset += page {
		runs, err := st.ListRuns(ctx, filter, page, offset)
		if err != nil {
			return nil, err
		}
		if len(runs) == 0 {
			break
		}
		for _, r := range runs {
			if r.Status == model.StatusRunning {
				continue
			}
			if len(refs) >= limit {
				break
			}
			project := r.Project
			if project == "" {
				project = "default"
			}
			refs = append(refs, RunRef{Project: project, ID: r.ID})
		}
		if len(runs) < page {
			break
		}
	}
	return refs, nil
}
