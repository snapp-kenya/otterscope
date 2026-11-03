// Package store owns SQLite persistence. All schema changes go through
// embedded, append-only migrations in migrations/.
package store

import (
	"context"
	"database/sql"
	"embed"
	"fmt"
	"io/fs"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	_ "modernc.org/sqlite"
)

// The store can hold prompts, tool payloads and message text, so its files
// must not be group- or world-readable. Directories we create are 0700 and
// the database (plus its -wal/-shm siblings) is 0600.
const (
	dbFileMode = 0o600
	dbDirMode  = 0o700
)

//go:embed migrations/*.sql
var migrationFS embed.FS

// Store wraps the SQLite database with two handles: a single-connection
// writer (SQLite serializes writes anyway; one connection avoids
// SQLITE_BUSY churn) and a small reader pool (WAL makes concurrent reads
// safe alongside the writer). Reads must go through the reader so a held
// rows cursor can never starve a write on the same call path (#22).
type Store struct {
	writer *sql.DB
	reader *sql.DB
	path   string // db file path, for backups
}

// Open opens (creating if needed) the database at path and applies pending
// migrations.
func Open(ctx context.Context, path string) (*Store, error) {
	// Lock the file down before SQLite can create it: modernc/sqlite (like
	// libsqlite3) creates the db world-readable under a normal umask, and
	// WAL/SHM inherit the db file's mode.
	if err := secureOpen(path); err != nil {
		return nil, err
	}

	// WAL for concurrent reads during ingest; busy_timeout so the UI and
	// ingest paths don't fail on transient write contention.
	dsn := fmt.Sprintf("file:%s?_pragma=journal_mode(WAL)&_pragma=busy_timeout(5000)&_pragma=foreign_keys(ON)", path)
	writer, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	writer.SetMaxOpenConns(1)

	reader, err := sql.Open("sqlite", dsn)
	if err != nil {
		writer.Close()
		return nil, err
	}
	reader.SetMaxOpenConns(4)

	s := &Store{writer: writer, reader: reader, path: path}
	if err := s.migrate(ctx); err != nil {
		s.Close()
		return nil, fmt.Errorf("migrate: %w", err)
	}
	// 0021 leaves pre-existing tokens to be hashed in Go (SQLite has no
	// SHA-256). This must finish before the store is handed out: until it
	// does, no legacy token can authenticate.
	if err := s.hashLegacyTokens(ctx); err != nil {
		s.Close()
		return nil, fmt.Errorf("migrate tokens: %w", err)
	}
	// The WAL and SHM are created lazily on the first write (migrations
	// guarantee one happened), so tighten them after migrating. This is also
	// what repairs a db whose files an older version left at 0644.
	if err := restrictPerms(path); err != nil {
		s.Close()
		return nil, err
	}
	return s, nil
}

// isMemoryPath reports paths with nothing on disk to protect.
func isMemoryPath(path string) bool {
	return path == "" || strings.Contains(path, ":memory:")
}

// secureOpen prepares the on-disk footprint before the database is opened:
// the parent directory is created 0700, the db file is created (or tightened)
// to 0600, and the WAL/SHM are tightened if they already exist.
func secureOpen(path string) error {
	if isMemoryPath(path) {
		return nil
	}
	dir := filepath.Dir(path)
	if dir != "" && dir != "." {
		_, statErr := os.Stat(dir)
		fresh := os.IsNotExist(statErr)
		if err := os.MkdirAll(dir, dbDirMode); err != nil {
			return fmt.Errorf("create database directory %s: %w", dir, err)
		}
		if err := tightenDir(dir); err != nil {
			if fresh {
				return err
			}
			// An existing directory may not be ours to re-permission (a
			// shared mount, another user's tree). The database file itself is
			// still locked down below, so this is not fatal.
			slog.Warn("could not tighten database directory permissions", "dir", dir, "err", err)
		}
	}
	// O_CREATE with an explicit 0600 mode is what keeps the file private
	// from the instant it exists (the umask can only strip bits, never add).
	f, err := os.OpenFile(path, os.O_RDWR|os.O_CREATE, dbFileMode)
	if err != nil {
		return fmt.Errorf("create database %s: %w", path, err)
	}
	if err := f.Close(); err != nil {
		return fmt.Errorf("create database %s: %w", path, err)
	}
	return restrictPerms(path)
}

// tightenDir forces dir to 0700, unless it is a location shared with other
// users or with the rest of the system (chmod'ing those would break unrelated
// software, and we do not own them).
func tightenDir(dir string) error {
	clean := filepath.Clean(dir)
	if clean == "/" || clean == filepath.Clean(os.TempDir()) {
		return nil
	}
	if home, err := os.UserHomeDir(); err == nil && clean == home {
		return nil
	}
	if err := os.Chmod(clean, dbDirMode); err != nil && !os.IsNotExist(err) {
		return fmt.Errorf("restrict database directory %s: %w", clean, err)
	}
	return nil
}

// restrictPerms makes the database and its WAL/SHM siblings owner-only.
// Missing siblings are normal: SQLite creates them on the first write and
// removes them on a clean close.
func restrictPerms(path string) error {
	if isMemoryPath(path) {
		return nil
	}
	for _, p := range []string{path, path + "-wal", path + "-shm"} {
		if err := os.Chmod(p, dbFileMode); err != nil && !os.IsNotExist(err) {
			return fmt.Errorf("restrict %s: %w", p, err)
		}
	}
	return nil
}

// Backup writes a consistent snapshot of the database to dest using SQLite's
// VACUUM INTO (includes committed WAL). dest must not already exist.
func (s *Store) Backup(ctx context.Context, dest string) error {
	if _, err := s.writer.ExecContext(ctx, "VACUUM INTO ?", dest); err != nil {
		return fmt.Errorf("backup to %s: %w", dest, err)
	}
	// VACUUM INTO creates dest with the process umask, so a snapshot of a
	// private database would otherwise land world-readable.
	if err := os.Chmod(dest, dbFileMode); err != nil {
		return fmt.Errorf("backup to %s: %w", dest, err)
	}
	return nil
}

// Close closes both database handles.
func (s *Store) Close() error {
	rerr := s.reader.Close()
	if werr := s.writer.Close(); werr != nil {
		return werr
	}
	return rerr
}

// DB exposes the raw writer handle for package-internal tests. It must not
// be used outside internal/.
func (s *Store) DB() *sql.DB { return s.writer }

func (s *Store) migrate(ctx context.Context) error {
	if _, err := s.writer.ExecContext(ctx,
		`CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT (datetime('now')))`); err != nil {
		return err
	}

	names, err := fs.Glob(migrationFS, "migrations/*.sql")
	if err != nil {
		return err
	}
	sort.Strings(names)

	// Count already-applied migrations: >0 means this is an existing db being
	// upgraded (not first init), so back it up before touching it.
	var applied int
	if err := s.writer.QueryRowContext(ctx, `SELECT COUNT(*) FROM schema_migrations`).Scan(&applied); err != nil {
		return err
	}
	pending := 0
	for _, name := range names {
		var done int
		if err := s.writer.QueryRowContext(ctx,
			`SELECT COUNT(*) FROM schema_migrations WHERE name = ?`, name).Scan(&done); err != nil {
			return err
		}
		if done == 0 {
			pending++
		}
	}
	if pending > 0 && applied > 0 {
		if err := s.backupBeforeMigrate(ctx); err != nil {
			// A failed pre-migration backup must not silently proceed to a
			// possibly-destructive migration.
			return fmt.Errorf("pre-migration backup failed (refusing to migrate): %w", err)
		}
	}

	for _, name := range names {
		var done int
		if err := s.writer.QueryRowContext(ctx,
			`SELECT COUNT(*) FROM schema_migrations WHERE name = ?`, name).Scan(&done); err != nil {
			return err
		}
		if done > 0 {
			continue
		}
		body, err := migrationFS.ReadFile(name)
		if err != nil {
			return err
		}
		tx, err := s.writer.BeginTx(ctx, nil)
		if err != nil {
			return err
		}
		if _, err := tx.ExecContext(ctx, string(body)); err != nil {
			tx.Rollback()
			return fmt.Errorf("apply %s: %w", name, err)
		}
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO schema_migrations (name) VALUES (?)`, name); err != nil {
			tx.Rollback()
			return err
		}
		if err := tx.Commit(); err != nil {
			return err
		}
	}
	return nil
}

// maxPreMigrationBackups is how many snapshots to keep. One is enough: it is
// the rollback point for the upgrade in progress, and older copies of a
// multi-gigabyte database must not accumulate.
const maxPreMigrationBackups = 1

// backupBeforeMigrate snapshots the db next to itself before an upgrade, so a
// bad migration on real data is recoverable. Skipped for in-memory dbs.
func (s *Store) backupBeforeMigrate(ctx context.Context) error {
	if isMemoryPath(s.path) {
		return nil
	}
	// Nanosecond precision: two opens in the same second must not collide on
	// the destination (VACUUM INTO refuses an existing file), and it makes
	// the newest backup unambiguous.
	dest := fmt.Sprintf("%s.pre-migrate-%d.bak", s.path, time.Now().UnixNano())
	if err := s.Backup(ctx, dest); err != nil {
		return err
	}
	// Prune only after the new snapshot exists, so a pruning bug can never
	// leave the upgrade with no backup at all. Failure to prune is logged,
	// not fatal: the migration still has its rollback point.
	if err := prunePreMigrationBackups(s.path, maxPreMigrationBackups); err != nil {
		slog.Warn("could not prune old pre-migration backups", "err", err)
	}
	slog.Info("backed up database before applying migrations", "backup", dest)
	return nil
}

// prunePreMigrationBackups keeps the keep newest .pre-migrate-*.bak files
// next to dbPath and deletes the rest.
func prunePreMigrationBackups(dbPath string, keep int) error {
	matches, err := filepath.Glob(dbPath + ".pre-migrate-*.bak")
	if err != nil {
		return err
	}
	type backup struct {
		path    string
		modTime time.Time
	}
	backups := make([]backup, 0, len(matches))
	for _, m := range matches {
		fi, err := os.Stat(m)
		if err != nil {
			if os.IsNotExist(err) {
				continue // pruned or removed concurrently
			}
			return err
		}
		backups = append(backups, backup{m, fi.ModTime()})
	}
	if len(backups) <= keep {
		return nil
	}
	// Newest first; the name breaks mtime ties (nanosecond-stamped names
	// sort in creation order).
	sort.Slice(backups, func(i, j int) bool {
		if backups[i].modTime.Equal(backups[j].modTime) {
			return backups[i].path > backups[j].path
		}
		return backups[i].modTime.After(backups[j].modTime)
	})
	for _, b := range backups[keep:] {
		if err := os.Remove(b.path); err != nil && !os.IsNotExist(err) {
			return fmt.Errorf("remove old backup %s: %w", b.path, err)
		}
	}
	return nil
}
