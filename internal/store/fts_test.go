package store

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/otterscope/otterscope/internal/model"
)

func stepWithMsg(id, runID, out string, sec int64) []model.Step {
	base := time.Unix(sec, 0)
	return []model.Step{
		{ID: runID + "-root", RunID: runID, Project: "default", Kind: model.StepAgent,
			Name: "invoke_agent a", Status: model.StatusOK, Start: base, End: base.Add(time.Second)},
		{ID: id, RunID: runID, ParentID: runID + "-root", Project: "default", Kind: model.StepLLM,
			Name: "chat m", Status: model.StatusOK, Start: base, End: base.Add(time.Second),
			LLM: &model.LLMCall{RequestModel: "m",
				OutputMessages: []model.Message{{Role: "assistant", Content: out}}}},
	}
}

func TestFullTextSearch(t *testing.T) {
	st := openTest(t)
	ctx := context.Background()
	if err := st.UpsertSteps(ctx, stepWithMsg("s1", "run-ship", "Your order A-1042 has shipped today", 1000)); err != nil {
		t.Fatal(err)
	}
	if err := st.UpsertSteps(ctx, stepWithMsg("s2", "run-refund", "I have processed your refund", 2000)); err != nil {
		t.Fatal(err)
	}

	// Phrase in one run's message.
	runs, err := st.ListRuns(ctx, Filter{Query: "shipped"}, 10, 0)
	if err != nil {
		t.Fatalf("search: %v", err)
	}
	if len(runs) != 1 || runs[0].ID != "run-ship" {
		t.Fatalf("search 'shipped' returned %+v", runs)
	}

	// Word only in the other.
	if runs, _ := st.ListRuns(ctx, Filter{Query: "refund"}, 10, 0); len(runs) != 1 || runs[0].ID != "run-refund" {
		t.Fatalf("search 'refund' returned %+v", runs)
	}

	// Punctuation-heavy token must not throw (FTS5 operators neutralized).
	if _, err := st.ListRuns(ctx, Filter{Query: "A-1042"}, 10, 0); err != nil {
		t.Fatalf("hyphenated query errored: %v", err)
	}
	if runs, _ := st.ListRuns(ctx, Filter{Query: "A-1042"}, 10, 0); len(runs) != 1 {
		t.Fatalf("search 'A-1042' returned %d, want 1", len(runs))
	}
	// Quote in query must not throw.
	if _, err := st.ListRuns(ctx, Filter{Query: `say "hi"`}, 10, 0); err != nil {
		t.Fatalf("quoted query errored: %v", err)
	}

	// Re-delivery keeps FTS in sync (no duplicate hit).
	if err := st.UpsertSteps(ctx, stepWithMsg("s1", "run-ship", "Your order A-1042 has shipped today", 1000)); err != nil {
		t.Fatal(err)
	}
	if runs, _ := st.ListRuns(ctx, Filter{Query: "shipped"}, 10, 0); len(runs) != 1 {
		t.Fatalf("re-delivery duplicated FTS hit: %d runs", len(runs))
	}

	// Stats respect the search filter too.
	stats, _ := st.GetStats(ctx, Filter{Query: "refund"})
	if stats.Runs != 1 {
		t.Fatalf("stats with search = %d runs, want 1", stats.Runs)
	}
}

// ftsRowCount returns the number of rows in the full-text table. It is a
// direct count, not a search: this is the table that holds the message text.
func ftsRowCount(t *testing.T, st *Store) int {
	t.Helper()
	var n int
	if err := st.DB().QueryRow(`SELECT count(*) FROM steps_fts`).Scan(&n); err != nil {
		t.Fatalf("count steps_fts: %v", err)
	}
	return n
}

// Retention must delete the FTS rows for swept runs: swapping the table in
// and out is not enough, because steps_fts has no foreign key to steps and
// keeps the full message text of deleted runs forever.
func TestSweepPurgesFullText(t *testing.T) {
	st := openTest(t)
	ctx := context.Background()

	if err := st.UpsertSteps(ctx, stepWithMsg("s1", "run-old", "Your order A-1042 has shipped today", 1000)); err != nil {
		t.Fatal(err)
	}
	if err := st.UpsertSteps(ctx, stepWithMsg("s2", "run-new", "I have processed your refund", 5000)); err != nil {
		t.Fatal(err)
	}
	if n := ftsRowCount(t, st); n != 4 { // two steps per run
		t.Fatalf("fts rows before sweep = %d, want 4", n)
	}

	// Sweep just the old run.
	n, err := st.Sweep(ctx, time.Unix(3000, 0))
	if err != nil {
		t.Fatalf("Sweep: %v", err)
	}
	if n != 1 {
		t.Fatalf("swept %d runs, want 1", n)
	}
	if got := ftsRowCount(t, st); got != 2 {
		t.Fatalf("fts rows after sweeping one run = %d, want 2 (only the live run)", got)
	}
	if runs, err := st.ListRuns(ctx, Filter{Query: "shipped"}, 10, 0); err != nil {
		t.Fatalf("search swept text: %v", err)
	} else if len(runs) != 0 {
		t.Fatalf("swept run still searchable: %+v", runs)
	}
	// The surviving run is untouched.
	if runs, _ := st.ListRuns(ctx, Filter{Query: "refund"}, 10, 0); len(runs) != 1 {
		t.Fatalf("live run lost from the index: %+v", runs)
	}

	// Sweep everything: no FTS rows may remain, and nothing may be findable.
	if n, err := st.Sweep(ctx, time.Unix(9000, 0)); err != nil || n != 1 {
		t.Fatalf("full sweep = %d runs, err=%v", n, err)
	}
	if got := ftsRowCount(t, st); got != 0 {
		t.Fatalf("fts rows after sweeping everything = %d, want 0", got)
	}
	for _, q := range []string{"shipped", "refund", "A-1042"} {
		if runs, err := st.ListRuns(ctx, Filter{Query: q}, 10, 0); err != nil {
			t.Fatalf("search %q: %v", q, err)
		} else if len(runs) != 0 {
			t.Fatalf("search %q returned %d runs after a full sweep", q, len(runs))
		}
	}
	if n, err := st.Sweep(ctx, time.Unix(9000, 0)); err != nil || n != 0 {
		t.Fatalf("second sweep = %d runs, err=%v (should be a no-op)", n, err)
	}
}

// The sweep must also drop FTS rows orphaned by an older release (deleted
// runs whose text was left in the index), so upgrading cleans up the leak
// that is already on disk.
func TestSweepPurgesOrphanedFullText(t *testing.T) {
	st := openTest(t)
	ctx := context.Background()

	if err := st.UpsertSteps(ctx, stepWithMsg("s1", "run-old", "Your order A-1042 has shipped today", 1000)); err != nil {
		t.Fatal(err)
	}
	// Reproduce the old behaviour: delete the run and its steps directly,
	// leaving the FTS rows behind.
	for _, q := range []string{
		`DELETE FROM steps WHERE run_id = 'run-old'`,
		`DELETE FROM runs WHERE id = 'run-old'`,
	} {
		if _, err := st.DB().ExecContext(ctx, q); err != nil {
			t.Fatal(err)
		}
	}
	if got := ftsRowCount(t, st); got != 2 {
		t.Fatalf("fts rows after manual delete = %d, want 2 (the leak)", got)
	}

	if _, err := st.Sweep(ctx, time.Unix(9000, 0)); err != nil {
		t.Fatalf("Sweep: %v", err)
	}
	if got := ftsRowCount(t, st); got != 0 {
		t.Fatalf("orphaned fts rows survived the sweep: %d", got)
	}
	if runs, err := st.ListRuns(ctx, Filter{Query: "shipped"}, 10, 0); err != nil {
		t.Fatalf("search: %v", err)
	} else if len(runs) != 0 {
		t.Fatalf("orphaned text still searchable: %+v", runs)
	}
}

// Deleting the rows is not enough on its own: the bytes stay in free pages
// until the database is reclaimed, so the sweep vacuums and truncates the WAL.
// Prove it by looking for the swept text in the files themselves.
func TestSweepReclaimsFileSpace(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "sweep.db")
	st, err := Open(ctx, path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer st.Close()

	const marker = "A-1042-ZQX"
	steps := stepWithMsg("s1", "run-old", "Your order "+marker+" has shipped today", 1000)
	if err := st.UpsertSteps(ctx, steps); err != nil {
		t.Fatal(err)
	}
	if !fileContains(t, marker, path, path+"-wal") {
		t.Fatal("setup: marker not on disk")
	}
	if _, err := st.Sweep(ctx, time.Unix(9000, 0)); err != nil {
		t.Fatalf("Sweep: %v", err)
	}
	if fileContains(t, marker, path, path+"-wal") {
		t.Fatal("swept message text still present in the database files")
	}
}

// fileContains reports whether any of paths contains needle as raw bytes.
func fileContains(t *testing.T, needle string, paths ...string) bool {
	t.Helper()
	for _, p := range paths {
		b, err := os.ReadFile(p)
		if err != nil {
			if os.IsNotExist(err) {
				continue
			}
			t.Fatalf("read %s: %v", p, err)
		}
		if bytes.Contains(b, []byte(needle)) {
			return true
		}
	}
	return false
}
