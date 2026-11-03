// Package config resolves serve configuration from (in increasing priority)
// built-in defaults, a JSON config file, and OTTERSCOPE_* environment
// variables. The caller then lets explicit CLI flags override the result, so
// the final precedence is flags > env > file > defaults.
package config

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/otterscope/otterscope/internal/basepath"
	"github.com/otterscope/otterscope/internal/ingest"
)

// Config holds all `serve` options. Durations are resolved to time.Duration.
type Config struct {
	DB       string
	Listen   string
	OTLP     string
	Pricing  string
	JudgeURL string
	// JudgeModels is the allow-list of models an llm_judge assertion may
	// name. An assertion chooses its own model, and assertions are writable
	// through the API, so without a list a caller would pick the price of
	// every judge call. Empty (the default) allows none: llm_judge
	// assertions are refused with an error naming the parameter to set.
	JudgeModels   []string
	Retention     time.Duration
	AlertInterval time.Duration
	ReadAuth      bool
	// IngestRate/IngestBurst bound how many OTLP batches one ingest key may
	// send per second. The built-in default is finite (50/s, burst 100) so an
	// exposed receiver is not a free amplifier; 0 disables the limit and has
	// to be asked for explicitly.
	IngestRate  float64
	IngestBurst float64
	// MaxSpansPerBatch/MaxTraceIDsPerBatch bound the work one OTLP request may
	// cause. They default to ingest.DefaultMaxSpansPerBatch and
	// ingest.DefaultMaxTraceIDsPerBatch; 0 also means "use the default", since
	// an unbounded batch is exactly the amplification this guards against.
	MaxSpansPerBatch    int
	MaxTraceIDsPerBatch int
	// BasePath mounts the whole application (UI, API, MCP, OTLP) under a
	// path prefix so it can sit behind path-based layer-7 routing. "" = root.
	BasePath string
}

// Default returns the built-in configuration.
func Default() Config {
	return Config{
		DB:            "otterscope.db",
		Listen:        "127.0.0.1:8317",
		OTLP:          "127.0.0.1:4318",
		JudgeURL:      "https://api.openai.com/v1",
		AlertInterval: time.Minute,
		// Finite by default: an exposed receiver must not accept unlimited
		// batch volume from one key.
		IngestRate:          50,
		IngestBurst:         100,
		MaxSpansPerBatch:    ingest.DefaultMaxSpansPerBatch,
		MaxTraceIDsPerBatch: ingest.DefaultMaxTraceIDsPerBatch,
	}
}

// fileConfig mirrors the JSON schema. Pointer fields distinguish "absent"
// (leave the current value) from "set to zero".
type fileConfig struct {
	DB            *string   `json:"db"`
	Listen        *string   `json:"listen"`
	OTLP          *string   `json:"otlp"`
	Pricing       *string   `json:"pricing"`
	JudgeURL      *string   `json:"judgeUrl"`
	JudgeModels   *[]string `json:"judgeModels"`
	Retention     *string   `json:"retention"`
	AlertInterval *string   `json:"alertInterval"`
	ReadAuth      *bool     `json:"readAuth"`
	IngestRate    *float64  `json:"ingestRate"`
	IngestBurst   *float64  `json:"ingestBurst"`
	// maxSpansPerBatch/maxTraceIdsPerBatch cap one OTLP request; see Config.
	MaxSpansPerBatch    *int    `json:"maxSpansPerBatch"`
	MaxTraceIDsPerBatch *int    `json:"maxTraceIdsPerBatch"`
	BasePath            *string `json:"basePath"`
}

// Load builds the effective config: defaults, then the JSON file at path (if
// non-empty), then OTTERSCOPE_* env vars via getenv.
func Load(path string, getenv func(string) string) (Config, error) {
	c := Default()
	if path != "" {
		if err := c.applyFile(path); err != nil {
			return Config{}, err
		}
	}
	if err := c.applyEnv(getenv); err != nil {
		return Config{}, err
	}
	if err := c.normalizeBasePath(); err != nil {
		return Config{}, err
	}
	return c, nil
}

func (c *Config) applyFile(path string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("read config %s: %w", path, err)
	}
	var f fileConfig
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&f); err != nil {
		return fmt.Errorf("parse config %s: %w", path, err)
	}
	setStr(&c.DB, f.DB)
	setStr(&c.Listen, f.Listen)
	setStr(&c.OTLP, f.OTLP)
	setStr(&c.Pricing, f.Pricing)
	setStr(&c.JudgeURL, f.JudgeURL)
	setStr(&c.BasePath, f.BasePath)
	if f.ReadAuth != nil {
		c.ReadAuth = *f.ReadAuth
	}
	if f.IngestRate != nil {
		c.IngestRate = *f.IngestRate
	}
	if f.IngestBurst != nil {
		c.IngestBurst = *f.IngestBurst
	}
	if f.MaxSpansPerBatch != nil {
		c.MaxSpansPerBatch = *f.MaxSpansPerBatch
	}
	if f.MaxTraceIDsPerBatch != nil {
		c.MaxTraceIDsPerBatch = *f.MaxTraceIDsPerBatch
	}
	if f.Retention != nil {
		d, err := time.ParseDuration(*f.Retention)
		if err != nil {
			return fmt.Errorf("config retention: %w", err)
		}
		c.Retention = d
	}
	if f.JudgeModels != nil {
		c.JudgeModels = cleanModels(*f.JudgeModels)
	}
	if f.AlertInterval != nil {
		d, err := time.ParseDuration(*f.AlertInterval)
		if err != nil {
			return fmt.Errorf("config alertInterval: %w", err)
		}
		c.AlertInterval = d
	}
	return nil
}

func (c *Config) applyEnv(getenv func(string) string) error {
	envStr(&c.DB, getenv, "OTTERSCOPE_DB")
	envStr(&c.Listen, getenv, "OTTERSCOPE_LISTEN")
	envStr(&c.OTLP, getenv, "OTTERSCOPE_OTLP")
	envStr(&c.Pricing, getenv, "OTTERSCOPE_PRICING")
	envStr(&c.JudgeURL, getenv, "OTTERSCOPE_JUDGE_URL")
	envStr(&c.BasePath, getenv, "OTTERSCOPE_BASE_PATH")
	if v := getenv("OTTERSCOPE_JUDGE_MODELS"); v != "" {
		c.JudgeModels = ParseModelList(v)
	}
	if v := getenv("OTTERSCOPE_READ_AUTH"); v != "" {
		b, err := strconv.ParseBool(v)
		if err != nil {
			return fmt.Errorf("OTTERSCOPE_READ_AUTH: %w", err)
		}
		c.ReadAuth = b
	}
	if err := envFloat(&c.IngestRate, getenv, "OTTERSCOPE_INGEST_RATE"); err != nil {
		return err
	}
	if err := envFloat(&c.IngestBurst, getenv, "OTTERSCOPE_INGEST_BURST"); err != nil {
		return err
	}
	if err := envInt(&c.MaxSpansPerBatch, getenv, "OTTERSCOPE_MAX_SPANS_PER_BATCH"); err != nil {
		return err
	}
	if err := envInt(&c.MaxTraceIDsPerBatch, getenv, "OTTERSCOPE_MAX_TRACE_IDS_PER_BATCH"); err != nil {
		return err
	}
	if err := envDur(&c.Retention, getenv, "OTTERSCOPE_RETENTION"); err != nil {
		return err
	}
	return envDur(&c.AlertInterval, getenv, "OTTERSCOPE_ALERT_INTERVAL")
}

// normalizeBasePath canonicalizes BasePath whatever its source, so "otterscope/",
// "/otterscope" and a config-file spelling all mean the same mount point.
func (c *Config) normalizeBasePath() error {
	p, err := basepath.Normalize(c.BasePath)
	if err != nil {
		return err
	}
	c.BasePath = p
	return nil
}

func setStr(dst *string, v *string) {
	if v != nil {
		*dst = *v
	}
}

// cleanModels trims a judge-model allow-list and drops empty entries, so
// "a, b," and ["a","b"] behave the same and a blank element cannot become a
// wildcard-ish match.
func cleanModels(in []string) []string {
	out := make([]string, 0, len(in))
	for _, m := range in {
		if m = strings.TrimSpace(m); m != "" {
			out = append(out, m)
		}
	}
	return out
}

// ParseModelList turns a comma-separated judge-model allow-list into its
// entries. Exported so the -judge-models flag and OTTERSCOPE_JUDGE_MODELS
// accept exactly the same spelling.
func ParseModelList(s string) []string { return cleanModels(strings.Split(s, ",")) }

func envStr(dst *string, getenv func(string) string, key string) {
	if v := getenv(key); v != "" {
		*dst = v
	}
}

func envFloat(dst *float64, getenv func(string) string, key string) error {
	v := getenv(key)
	if v == "" {
		return nil
	}
	f, err := strconv.ParseFloat(v, 64)
	if err != nil {
		return fmt.Errorf("%s: %w", key, err)
	}
	*dst = f
	return nil
}

func envInt(dst *int, getenv func(string) string, key string) error {
	v := getenv(key)
	if v == "" {
		return nil
	}
	n, err := strconv.Atoi(v)
	if err != nil {
		return fmt.Errorf("%s: %w", key, err)
	}
	*dst = n
	return nil
}

func envDur(dst *time.Duration, getenv func(string) string, key string) error {
	v := getenv(key)
	if v == "" {
		return nil
	}
	d, err := time.ParseDuration(v)
	if err != nil {
		return fmt.Errorf("%s: %w", key, err)
	}
	*dst = d
	return nil
}
