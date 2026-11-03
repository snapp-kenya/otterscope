package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// writeConfig drops a JSON config file in a temp dir and returns its path.
func writeConfig(t *testing.T, body string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "config.json")
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatalf("write config: %v", err)
	}
	return path
}

// TestBasePathFromFile: the JSON key is normalized like every other source, so
// a trailing slash in a config file means the same mount point as the flag.
func TestBasePathFromFile(t *testing.T) {
	c, err := Load(writeConfig(t, `{"basePath":"/otterscope/"}`), noEnv)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if c.BasePath != "/otterscope" {
		t.Fatalf("BasePath = %q, want /otterscope", c.BasePath)
	}
}

// TestBasePathFromEnv: no file at all, so the env var is the only source.
func TestBasePathFromEnv(t *testing.T) {
	c, err := Load("", func(k string) string {
		if k == "OTTERSCOPE_BASE_PATH" {
			return "otterscope" // deliberately unslashed: still the same mount
		}
		return ""
	})
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if c.BasePath != "/otterscope" {
		t.Fatalf("BasePath = %q, want /otterscope", c.BasePath)
	}
}

// TestBasePathEnvOverridesFile pins the documented precedence for this setting
// specifically: env beats file, file beats the default.
func TestBasePathEnvOverridesFile(t *testing.T) {
	path := writeConfig(t, `{"basePath":"/from-file"}`)
	c, err := Load(path, func(k string) string {
		if k == "OTTERSCOPE_BASE_PATH" {
			return "/otterscope"
		}
		return ""
	})
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if c.BasePath != "/otterscope" {
		t.Fatalf("BasePath = %q, want the env value /otterscope", c.BasePath)
	}
}

// TestBasePathInvalidIsRejected: a base path that cannot be a single URL-path
// prefix is a startup error, not a mount that silently matches nothing (or
// worse, matches other applications' paths).
func TestBasePathInvalidIsRejected(t *testing.T) {
	for _, body := range []string{
		`{"basePath":"/a//b"}`,
		`{"basePath":"/a/../b"}`,
		`{"basePath":"/a?b"}`,
		`{"basePath":"/otter scope"}`,
	} {
		_, err := Load(writeConfig(t, body), noEnv)
		if err == nil {
			t.Errorf("Load(%s) accepted an invalid base path", body)
			continue
		}
		// The message must name the offending setting, or an operator seeing
		// it has no idea which knob to fix.
		if !strings.Contains(err.Error(), "base path") {
			t.Errorf("Load(%s) error = %q, want it to mention the base path", body, err)
		}
	}

	// The same rejection applies when the value arrives through the env var.
	if _, err := Load("", func(k string) string {
		if k == "OTTERSCOPE_BASE_PATH" {
			return "/a//b"
		}
		return ""
	}); err == nil {
		t.Error("Load accepted an invalid OTTERSCOPE_BASE_PATH")
	}
}

// TestBasePathDefaultsToRoot: unset means "" — the listener root, i.e. exactly
// the routing an existing deployment expects.
func TestBasePathDefaultsToRoot(t *testing.T) {
	c, err := Load("", noEnv)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if c.BasePath != "" {
		t.Fatalf("BasePath = %q, want \"\" (root)", c.BasePath)
	}
	// Explicit root spellings collapse to the same value.
	for _, raw := range []string{"/", "   "} {
		c, err := Load(writeConfig(t, `{"basePath":"`+raw+`"}`), noEnv)
		if err != nil {
			t.Fatalf("Load basePath=%q: %v", raw, err)
		}
		if c.BasePath != "" {
			t.Errorf("basePath %q normalized to %q, want \"\"", raw, c.BasePath)
		}
	}
}
