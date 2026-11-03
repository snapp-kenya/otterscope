package store

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/otterscope/otterscope/internal/model"
)

func statSize(t *testing.T, path string) int64 {
	t.Helper()
	fi, err := os.Stat(path)
	if err != nil {
		if os.IsNotExist(err) {
			return 0
		}
		t.Fatalf("stat %s: %v", path, err)
	}
	return fi.Size()
}

// In WAL mode most (sometimes all) of the live data sits in the -wal file
// until a checkpoint, so the reported db size must include it. The old metric
// stat'ed the main file only and reported 4 KiB for a database holding ~2 MB.
func TestMetricsIncludesWAL(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "size.db")
	st, err := Open(ctx, path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer st.Close()

	// Keep the WAL from being folded back into the main file, so the test
	// exercises exactly the case the metric used to miss.
	if _, err := st.DB().ExecContext(ctx, `PRAGMA wal_autocheckpoint = 0`); err != nil {
		t.Fatalf("disable autocheckpoint: %v", err)
	}

	out := strings.Repeat("tool output for ticket A-1042 ", 200) // ~5.4 KB/step
	for i := 0; i < 40; i++ {
		runID := fmt.Sprintf("run-%d", i)
		steps := []model.Step{
			{ID: runID + "-root", RunID: runID, Project: "default", Kind: model.StepAgent,
				Name: "invoke_agent a", Status: model.StatusOK, Start: ts(1000 + int64(i)), End: ts(1001 + int64(i))},
			{ID: runID + "-llm", RunID: runID, ParentID: runID + "-root",
				Project: "default", Kind: model.StepLLM, Name: "chat m", Status: model.StatusOK,
				Start: ts(1000 + int64(i)), End: ts(1001 + int64(i)),
				LLM: &model.LLMCall{RequestModel: "m",
					OutputMessages: []model.Message{{Role: "assistant", Content: out}}}},
		}
		if err := st.UpsertSteps(ctx, steps); err != nil {
			t.Fatalf("UpsertSteps %d: %v", i, err)
		}
	}

	mainSize := statSize(t, path)
	walSize := statSize(t, path+"-wal")
	shmSize := statSize(t, path+"-shm")
	if walSize == 0 {
		t.Fatal("expected a non-empty WAL; the test setup is not exercising the case")
	}
	if walSize <= mainSize {
		t.Fatalf("WAL (%d B) should hold more than the main file (%d B) here", walSize, mainSize)
	}

	m, err := st.Metrics(ctx)
	if err != nil {
		t.Fatalf("Metrics: %v", err)
	}
	if want := mainSize + walSize + shmSize; m.DBSizeBytes != want {
		t.Fatalf("DBSizeBytes = %d, want %d (main %d + wal %d + shm %d)",
			m.DBSizeBytes, want, mainSize, walSize, shmSize)
	}
	if m.DBSizeBytes <= mainSize {
		t.Fatalf("DBSizeBytes = %d does not account for the %d B in the WAL", m.DBSizeBytes, walSize)
	}
}

// An in-memory store has no file to measure; it must report zero rather than
// whatever a stray path might name.
func TestDBSizeInMemoryIsZero(t *testing.T) {
	for _, p := range []string{"", ":memory:"} {
		if got := dbSizeBytes(p); got != 0 {
			t.Fatalf("dbSizeBytes(%q) = %d, want 0", p, got)
		}
	}
}
