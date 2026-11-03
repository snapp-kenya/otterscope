package store

import (
	"context"
	"database/sql"
	"os"
	"path/filepath"
	"testing"

	_ "modernc.org/sqlite"
)

func TestBackupProducesValidCopy(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	st, err := Open(ctx, filepath.Join(dir, "src.db"))
	if err != nil {
		t.Fatal(err)
	}
	if err := st.UpsertSteps(ctx, sampleRun("r1", 1000)); err != nil {
		t.Fatal(err)
	}

	dest := filepath.Join(dir, "backup.db")
	if err := st.Backup(ctx, dest); err != nil {
		t.Fatalf("Backup: %v", err)
	}
	st.Close()

	// The backup opens as a valid store with the data intact.
	restored, err := Open(ctx, dest)
	if err != nil {
		t.Fatalf("open backup: %v", err)
	}
	defer restored.Close()
	runs, err := restored.ListRuns(ctx, Filter{}, 10, 0)
	if err != nil || len(runs) != 1 || runs[0].ID != "r1" {
		t.Fatalf("backup missing data: %+v err=%v", runs, err)
	}
}

func TestAutoBackupBeforeMigrate(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "up.db")

	// Fully migrate, add data.
	st, err := Open(ctx, path)
	if err != nil {
		t.Fatal(err)
	}
	if err := st.UpsertSteps(ctx, sampleRun("r1", 1000)); err != nil {
		t.Fatal(err)
	}
	// Simulate being one migration behind so the next Open sees a pending
	// migration on an existing (non-fresh) db. Re-applying it fails (its
	// objects already exist) — that's fine: the point is that the backup must
	// be taken BEFORE the migration is attempted.
	//
	// Drop the *first* migration's record, not the last: 0001 creates a table
	// unconditionally, so re-applying it always fails. Whichever migration
	// happens to be last is not guaranteed to — a table-rebuild migration
	// drops and recreates its own objects, so it re-applies cleanly and this
	// test would silently stop testing anything.
	if _, err := st.DB().ExecContext(ctx,
		`DELETE FROM schema_migrations WHERE name = (SELECT min(name) FROM schema_migrations)`); err != nil {
		t.Fatal(err)
	}
	st.Close()

	if n := len(backupFiles(t, path)); n != 0 {
		t.Fatalf("unexpected pre-existing backups: %d", n)
	}

	// Reopen: migrate backs up, then the (artificial) re-apply fails.
	_, err = Open(ctx, path)
	if err == nil {
		t.Fatal("expected the re-applied migration to fail in this setup")
	}

	// The backup must exist despite the failed migration, and hold the
	// pre-migration data. Query it with a raw connection so we don't
	// re-trigger migration on the snapshot.
	baks := backupFiles(t, path)
	if len(baks) != 1 {
		t.Fatalf("expected exactly one pre-migration backup, got %d", len(baks))
	}
	raw, err := sql.Open("sqlite", "file:"+baks[0])
	if err != nil {
		t.Fatal(err)
	}
	defer raw.Close()
	var n int
	if err := raw.QueryRowContext(ctx, `SELECT count(*) FROM runs`).Scan(&n); err != nil {
		t.Fatalf("query backup: %v", err)
	}
	if n != 1 {
		t.Fatalf("backup didn't capture pre-migration data: %d runs", n)
	}
}

func TestNoBackupOnFreshInit(t *testing.T) {
	path := filepath.Join(t.TempDir(), "fresh.db")
	st, err := Open(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()
	// First init applies all migrations but must NOT create a .bak (nothing
	// to protect yet).
	if n := len(backupFiles(t, path)); n != 0 {
		t.Fatalf("fresh init created %d backups, want 0", n)
	}
}

// A snapshot is a full copy of a database that holds prompts and message
// text, so it must be as private as the database itself (VACUUM INTO
// otherwise honours the umask and writes 0644).
func TestBackupFileIsPrivate(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	st, err := Open(ctx, filepath.Join(dir, "src.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	dest := filepath.Join(dir, "snapshot.db")
	if err := st.Backup(ctx, dest); err != nil {
		t.Fatalf("Backup: %v", err)
	}
	assertMode(t, dest, 0o600)
}

// Every schema upgrade writes a full database copy next to the db. Keeping
// them all lets a long-lived instance mirror its database many times over, so
// only the newest may survive — and it must be the newest, not just any.
func TestPreMigrationBackupsPruneToNewest(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "up.db")
	st, err := Open(ctx, path)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()
	if err := st.UpsertSteps(ctx, sampleRun("r1", 1000)); err != nil {
		t.Fatal(err)
	}

	var newest string
	for i := 0; i < 3; i++ {
		before := backupFiles(t, path)
		if err := st.backupBeforeMigrate(ctx); err != nil {
			t.Fatalf("backup #%d: %v", i+1, err)
		}
		after := backupFiles(t, path)
		newest = newBackup(before, after)
		if newest == "" {
			t.Fatalf("backup #%d created no new snapshot (have %v)", i+1, after)
		}
		if got := len(after); got != 1 {
			t.Fatalf("after backup #%d: %d backups on disk, want 1 (%v)", i+1, got, after)
		}
	}

	baks := backupFiles(t, path)
	if len(baks) != 1 {
		t.Fatalf("kept %d backups, want 1: %v", len(baks), baks)
	}
	if baks[0] != newest {
		t.Fatalf("kept %q, want the newest %q", baks[0], newest)
	}
	assertMode(t, baks[0], 0o600)

	// The survivor is still a valid, complete snapshot.
	raw, err := sql.Open("sqlite", "file:"+baks[0])
	if err != nil {
		t.Fatal(err)
	}
	defer raw.Close()
	var n int
	if err := raw.QueryRowContext(ctx, `SELECT count(*) FROM runs`).Scan(&n); err != nil {
		t.Fatalf("query backup: %v", err)
	}
	if n != 1 {
		t.Fatalf("kept backup holds %d runs, want 1", n)
	}
}

// newBackup returns the path present in after but not in before.
func newBackup(before, after []string) string {
	seen := make(map[string]bool, len(before))
	for _, b := range before {
		seen[b] = true
	}
	for _, a := range after {
		if !seen[a] {
			return a
		}
	}
	return ""
}

func backupFiles(t *testing.T, dbPath string) []string {
	t.Helper()
	matches, err := filepath.Glob(dbPath + ".pre-migrate-*.bak")
	if err != nil {
		t.Fatal(err)
	}
	var out []string
	for _, m := range matches {
		if _, err := os.Stat(m); err == nil {
			out = append(out, m)
		}
	}
	return out
}
