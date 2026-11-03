package store

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"log/slog"
	"time"
)

// Project is an isolation dimension: each has its own ingest key.
type Project struct {
	Name      string
	IngestKey string
	CreatedAt time.Time
}

// CreateProject adds a project with a fresh random ingest key and returns it.
func (s *Store) CreateProject(ctx context.Context, name string) (Project, error) {
	keyBytes := make([]byte, 16)
	if _, err := rand.Read(keyBytes); err != nil {
		return Project{}, err
	}
	p := Project{Name: name, IngestKey: hex.EncodeToString(keyBytes), CreatedAt: time.Now()}
	_, err := s.writer.ExecContext(ctx,
		`INSERT INTO projects (name, ingest_key, created_ns) VALUES (?,?,?)`,
		p.Name, p.IngestKey, p.CreatedAt.UnixNano())
	if err != nil {
		return Project{}, fmt.Errorf("create project %q: %w", name, err)
	}
	s.audit(ctx, "create", "project", name)
	return p, nil
}

// ListProjects returns all projects, oldest first.
func (s *Store) ListProjects(ctx context.Context) ([]Project, error) {
	rows, err := s.reader.QueryContext(ctx,
		`SELECT name, ingest_key, created_ns FROM projects ORDER BY id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Project
	for rows.Next() {
		var p Project
		var ns int64
		if err := rows.Scan(&p.Name, &p.IngestKey, &ns); err != nil {
			return nil, err
		}
		p.CreatedAt = time.Unix(0, ns)
		out = append(out, p)
	}
	return out, rows.Err()
}

// ProjectForKey resolves an ingest key to its project name. ok=false means
// the key is unknown. The empty key always resolves to "default".
func (s *Store) ProjectForKey(ctx context.Context, key string) (string, bool) {
	if key == "" {
		return "default", true
	}
	var name string
	err := s.reader.QueryRowContext(ctx,
		`SELECT name FROM projects WHERE ingest_key = ?`, key).Scan(&name)
	if errors.Is(err, sql.ErrNoRows) {
		return "", false
	}
	if err != nil {
		return "", false
	}
	return name, true
}

// Sweep deletes runs (with their steps) started before cutoff and raw
// batches received before cutoff. Returns the number of runs removed.
func (s *Store) Sweep(ctx context.Context, cutoff time.Time) (int64, error) {
	tx, err := s.writer.BeginTx(ctx, nil)
	if err != nil {
		return 0, err
	}
	defer tx.Rollback()

	ns := cutoff.UnixNano()
	// steps_fts is a standalone FTS5 table with no foreign key to steps, so
	// deleting steps leaves the full text of every message and tool payload
	// behind — the retention sweep used to "delete" runs while their content
	// stayed searchable and on disk forever. Delete the FTS rows first, while
	// the steps they describe are still there to identify them.
	ftsRes, err := tx.ExecContext(ctx, `
		DELETE FROM steps_fts
		 WHERE (project, step_id) IN (
			SELECT CASE WHEN project = '' THEN 'default' ELSE project END, id
			  FROM steps WHERE run_id IN (SELECT id FROM runs WHERE start_ns < ?1))`,
		ns)
	if err != nil {
		return 0, fmt.Errorf("sweep fts rows: %w", err)
	}
	// Also drop rows that already lost their step (deleted by an older
	// version of this sweep, or left by a failed ingest). They can never be
	// returned by a search — the search joins runs — but they still hold
	// message text, so they must not survive a sweep on an existing db.
	if _, err := tx.ExecContext(ctx, `
		DELETE FROM steps_fts
		 WHERE NOT EXISTS (
			SELECT 1 FROM steps s
			 WHERE s.id = steps_fts.step_id
			   AND CASE WHEN s.project = '' THEN 'default' ELSE s.project END = steps_fts.project)`); err != nil {
		return 0, fmt.Errorf("sweep orphaned fts rows: %w", err)
	}
	if _, err := tx.ExecContext(ctx,
		`DELETE FROM assertion_results WHERE run_id IN (SELECT id FROM runs WHERE start_ns < ?)`, ns); err != nil {
		return 0, err
	}
	if _, err := tx.ExecContext(ctx,
		`DELETE FROM steps WHERE run_id IN (SELECT id FROM runs WHERE start_ns < ?)`, ns); err != nil {
		return 0, err
	}
	res, err := tx.ExecContext(ctx, `DELETE FROM runs WHERE start_ns < ?`, ns)
	if err != nil {
		return 0, err
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM raw_batches WHERE received_ns < ?`, ns); err != nil {
		return 0, err
	}
	n, _ := res.RowsAffected()
	ftsN, _ := ftsRes.RowsAffected()
	if err := tx.Commit(); err != nil {
		return 0, err
	}
	if n > 0 || ftsN > 0 {
		s.reclaim(ctx)
	}
	return n, nil
}

// reclaim gives back the space a sweep freed. Deleting rows leaves their
// bytes in the file's free pages and in the WAL until the database is
// rewritten and the WAL checkpointed, so without this the swept message text
// is still sitting in a (0600) file — and the file never shrinks.
//
// Best effort by design: the deletions are already committed, so a reader
// holding a lock must not turn a successful sweep into an error.
func (s *Store) reclaim(ctx context.Context) {
	if _, err := s.writer.ExecContext(ctx, `INSERT INTO steps_fts(steps_fts) VALUES('optimize')`); err != nil {
		slog.Warn("sweep: fts optimize failed", "err", err)
	}
	if _, err := s.writer.ExecContext(ctx, `VACUUM`); err != nil {
		slog.Warn("sweep: vacuum failed", "err", err)
		return
	}
	// VACUUM leaves the (now useless) WAL behind in WAL mode; truncate it so
	// the pre-sweep pages aren't kept on disk either.
	if _, err := s.writer.ExecContext(ctx, `PRAGMA wal_checkpoint(TRUNCATE)`); err != nil {
		slog.Warn("sweep: wal checkpoint failed", "err", err)
	}
}
