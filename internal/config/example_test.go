package config

import (
	"testing"
	"time"

	"github.com/otterscope/otterscope/internal/ingest"
)

// The documented example config must keep loading. applyFile uses
// DisallowUnknownFields, so renaming or removing a fileConfig json tag turns
// docs/otterscope.example.json into a file that errors on startup — and the
// person who finds out is a new user on their first `serve` (#106).
func TestExampleConfigLoads(t *testing.T) {
	const path = "../../docs/otterscope.example.json"
	cfg, err := Load(path, func(string) string { return "" })
	if err != nil {
		t.Fatalf("%s does not load: %v", path, err)
	}

	// Spot-check keys across each conversion path (string, bool, float,
	// duration) so a silently-ignored key is caught too.
	if cfg.DB != "/data/otterscope.db" {
		t.Errorf("db = %q, not read from the example", cfg.DB)
	}
	if !cfg.ReadAuth {
		t.Error("readAuth = false, not read from the example")
	}
	if cfg.IngestRate != 50 {
		t.Errorf("ingestRate = %v, not read from the example", cfg.IngestRate)
	}
	if cfg.MaxSpansPerBatch != ingest.DefaultMaxSpansPerBatch {
		t.Errorf("maxSpansPerBatch = %v, not read from the example", cfg.MaxSpansPerBatch)
	}
	if cfg.MaxTraceIDsPerBatch != ingest.DefaultMaxTraceIDsPerBatch {
		t.Errorf("maxTraceIdsPerBatch = %v, not read from the example", cfg.MaxTraceIDsPerBatch)
	}
	if cfg.Retention != 2160*time.Hour {
		t.Errorf("retention = %v, not read from the example", cfg.Retention)
	}
	if cfg.AlertInterval != time.Minute {
		t.Errorf("alertInterval = %v, not read from the example", cfg.AlertInterval)
	}
	if len(cfg.JudgeModels) != 1 || cfg.JudgeModels[0] != "gpt-4o-mini" {
		t.Errorf("judgeModels = %v, not read from the example", cfg.JudgeModels)
	}
}
