package store

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

func assertMode(t *testing.T, path string, want os.FileMode) {
	t.Helper()
	fi, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat %s: %v", path, err)
	}
	if got := fi.Mode().Perm(); got != want {
		t.Fatalf("%s mode = %04o, want %04o", filepath.Base(path), got, want)
	}
}

// A fresh store must not be readable by other users: it holds prompts, tool
// payloads and message text. The umask-based default (0644/0755) leaks all of
// it on a multi-user host or a shared volume.
func TestOpenCreatesPrivateFiles(t *testing.T) {
	ctx := context.Background()
	dir := filepath.Join(t.TempDir(), "data") // not created yet
	path := filepath.Join(dir, "otter.db")

	st, err := Open(ctx, path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer st.Close()

	// A write guarantees the WAL (and its SHM index) exist on disk.
	if err := st.UpsertSteps(ctx, sampleRun("r1", 1000)); err != nil {
		t.Fatalf("UpsertSteps: %v", err)
	}

	assertMode(t, dir, 0o700)
	assertMode(t, path, 0o600)
	for _, suffix := range []string{"-wal", "-shm"} {
		assertMode(t, path+suffix, 0o600)
	}
}

// A database created by an older version (0644 file in a 0755 directory) must
// be tightened on the next open instead of staying world-readable forever.
func TestOpenTightensExistingWideFiles(t *testing.T) {
	ctx := context.Background()
	dir := filepath.Join(t.TempDir(), "data")
	path := filepath.Join(dir, "otter.db")

	st, err := Open(ctx, path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if err := st.UpsertSteps(ctx, sampleRun("r1", 1000)); err != nil {
		t.Fatalf("UpsertSteps: %v", err)
	}
	if err := st.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	// Simulate the wide permissions of the previous release.
	for _, p := range []string{path, path + "-wal", path + "-shm"} {
		if _, err := os.Stat(p); err == nil {
			if err := os.Chmod(p, 0o644); err != nil {
				t.Fatal(err)
			}
		}
	}
	if err := os.Chmod(dir, 0o755); err != nil {
		t.Fatal(err)
	}

	st2, err := Open(ctx, path)
	if err != nil {
		t.Fatalf("reopen: %v", err)
	}
	defer st2.Close()
	// Re-check after a write, so a recreated WAL is covered too.
	if err := st2.UpsertSteps(ctx, sampleRun("r2", 2000)); err != nil {
		t.Fatalf("UpsertSteps: %v", err)
	}

	assertMode(t, dir, 0o700)
	assertMode(t, path, 0o600)
	for _, suffix := range []string{"-wal", "-shm"} {
		assertMode(t, path+suffix, 0o600)
	}
}

// The pre-open tightening must never chmod a directory we do not own: a db
// path pointing into a shared location would otherwise re-permission it.
func TestTightenDirSkipsSharedLocations(t *testing.T) {
	for _, dir := range []string{os.TempDir(), "/"} {
		fi, err := os.Stat(dir)
		if err != nil {
			t.Fatal(err)
		}
		before := fi.Mode().Perm()
		if err := tightenDir(dir); err != nil {
			t.Fatalf("tightenDir(%s): %v", dir, err)
		}
		fi, err = os.Stat(dir)
		if err != nil {
			t.Fatal(err)
		}
		if got := fi.Mode().Perm(); got != before {
			t.Fatalf("tightenDir(%s) changed mode %04o → %04o", dir, before, got)
		}
	}

	// A directory the store manages is tightened.
	own := t.TempDir()
	if err := os.Chmod(own, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := tightenDir(own); err != nil {
		t.Fatalf("tightenDir: %v", err)
	}
	assertMode(t, own, 0o700)
}
