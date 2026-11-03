package store

import (
	"context"
	"os"
)

// Metrics is a point-in-time snapshot of instance counts for /metrics.
type Metrics struct {
	Runs         int64
	RunsOK       int64
	RunsError    int64
	RunsRunning  int64
	Steps        int64
	Projects     int64
	AlertsFiring int64
	DBSizeBytes  int64
}

// Metrics gathers instance counts (queried on demand, so Prometheus can
// derive rates from successive scrapes).
func (s *Store) Metrics(ctx context.Context) (Metrics, error) {
	var m Metrics
	err := s.reader.QueryRowContext(ctx, `
		SELECT count(*),
		       coalesce(sum(status='ok'),0),
		       coalesce(sum(status='error'),0),
		       coalesce(sum(status='running'),0)
		FROM runs`).Scan(&m.Runs, &m.RunsOK, &m.RunsError, &m.RunsRunning)
	if err != nil {
		return Metrics{}, err
	}
	if err := s.reader.QueryRowContext(ctx, `SELECT count(*) FROM steps`).Scan(&m.Steps); err != nil {
		return Metrics{}, err
	}
	if err := s.reader.QueryRowContext(ctx, `SELECT count(*) FROM projects`).Scan(&m.Projects); err != nil {
		return Metrics{}, err
	}
	if err := s.reader.QueryRowContext(ctx, `SELECT coalesce(sum(firing),0) FROM alerts`).Scan(&m.AlertsFiring); err != nil {
		return Metrics{}, err
	}
	m.DBSizeBytes = dbSizeBytes(s.path)
	return m, nil
}

// dbSizeBytes is the store's real on-disk footprint: the main database file
// plus the -wal and -shm siblings. In WAL mode almost all recent data — and
// often the whole database, on a busy instance — lives in the WAL, so
// reporting the main file alone understates the size by orders of magnitude.
func dbSizeBytes(path string) int64 {
	if isMemoryPath(path) {
		return 0
	}
	var total int64
	for _, p := range []string{path, path + "-wal", path + "-shm"} {
		if fi, err := os.Stat(p); err == nil {
			total += fi.Size()
		}
	}
	return total
}
