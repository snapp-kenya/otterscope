package config

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func noEnv(string) string { return "" }

func TestDefaults(t *testing.T) {
	c, err := Load("", noEnv)
	if err != nil {
		t.Fatal(err)
	}
	if c.DB != "otterscope.db" || c.Listen != "127.0.0.1:8317" || c.AlertInterval != time.Minute {
		t.Fatalf("defaults wrong: %+v", c)
	}
	// The rate limit must be finite out of the box, or an exposed receiver is
	// a free amplifier.
	if c.IngestRate <= 0 || c.IngestBurst <= 0 {
		t.Fatalf("ingest rate limit must default to a finite value, got rate=%v burst=%v", c.IngestRate, c.IngestBurst)
	}
	if c.MaxSpansPerBatch <= 0 || c.MaxTraceIDsPerBatch <= 0 {
		t.Fatalf("per-batch caps must default to finite values, got %d/%d", c.MaxSpansPerBatch, c.MaxTraceIDsPerBatch)
	}
	// Fail closed: an assertion may not name a judge model until an operator
	// allows one, or a caller picks the price of every judge call.
	if len(c.JudgeModels) != 0 {
		t.Fatalf("judge model allow-list must default to empty, got %v", c.JudgeModels)
	}
}

func TestJudgeModelsFromEnv(t *testing.T) {
	env := map[string]string{"OTTERSCOPE_JUDGE_MODELS": " gpt-4o-mini, ,o3-mini "}
	c, err := Load("", func(k string) string { return env[k] })
	if err != nil {
		t.Fatal(err)
	}
	if len(c.JudgeModels) != 2 || c.JudgeModels[0] != "gpt-4o-mini" || c.JudgeModels[1] != "o3-mini" {
		t.Fatalf("judge models = %v, want the two named models with blanks dropped", c.JudgeModels)
	}
}

func TestFileOverlaysDefaults(t *testing.T) {
	path := filepath.Join(t.TempDir(), "c.json")
	os.WriteFile(path, []byte(`{
		"db": "/data/x.db",
		"listen": ":8317",
		"retention": "720h",
		"ingestRate": 50,
		"readAuth": true
	}`), 0o644)

	c, err := Load(path, noEnv)
	if err != nil {
		t.Fatal(err)
	}
	if c.DB != "/data/x.db" || c.Listen != ":8317" {
		t.Errorf("file not applied: %+v", c)
	}
	if c.Retention != 720*time.Hour || c.IngestRate != 50 || !c.ReadAuth {
		t.Errorf("typed fields: %+v", c)
	}
	// Unset file fields keep defaults.
	if c.OTLP != "127.0.0.1:4318" {
		t.Errorf("unset field lost default: %q", c.OTLP)
	}
}

func TestEnvOverridesFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "c.json")
	os.WriteFile(path, []byte(`{"db":"/from/file.db","ingestRate":10}`), 0o644)
	env := map[string]string{
		"OTTERSCOPE_DB":          "/from/env.db",
		"OTTERSCOPE_INGEST_RATE": "99",
		"OTTERSCOPE_RETENTION":   "24h",
	}
	c, err := Load(path, func(k string) string { return env[k] })
	if err != nil {
		t.Fatal(err)
	}
	if c.DB != "/from/env.db" || c.IngestRate != 99 || c.Retention != 24*time.Hour {
		t.Fatalf("env should override file: %+v", c)
	}
}

func TestBadFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "c.json")
	os.WriteFile(path, []byte(`{"unknown_field": 1}`), 0o644)
	if _, err := Load(path, noEnv); err == nil {
		t.Fatal("unknown field should error")
	}
	os.WriteFile(path, []byte(`{"retention":"nonsense"}`), 0o644)
	if _, err := Load(path, noEnv); err == nil {
		t.Fatal("bad duration should error")
	}
}
