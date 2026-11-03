package store

import (
	"context"
	"fmt"
	"time"

	"github.com/otterscope/otterscope/internal/model"
)

// IngestBatch persists a raw ingested batch and its normalized steps in one
// transaction. raw is the gzip-compressed protobuf of the original request.
//
// It returns the number of steps refused because an existing (project, span
// ID) came from a different resource service — the caller reports those to the
// OTLP client as rejected rather than silently replacing another service's
// step. The raw payload is still stored only when something was accepted, so a
// batch that was entirely rejected cannot be replayed later by `renormalize`.
func (s *Store) IngestBatch(ctx context.Context, project string, raw []byte, steps []model.Step) (int, error) {
	tx, err := s.writer.BeginTx(ctx, nil)
	if err != nil {
		return 0, err
	}
	defer tx.Rollback()

	rejected, err := upsertStepsTx(ctx, tx, steps)
	if err != nil {
		return 0, err
	}
	if len(steps) > 0 && rejected == len(steps) {
		// Nothing was written; keep no replayable copy of a refused batch.
		return rejected, tx.Commit()
	}
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO raw_batches (received_ns, payload, project) VALUES (?, ?, ?)`,
		time.Now().UnixNano(), raw, project); err != nil {
		return 0, fmt.Errorf("insert raw batch: %w", err)
	}
	return rejected, tx.Commit()
}

// EachRawBatch streams every stored raw payload to fn in insertion order,
// stopping on the first error.
//
// Payloads are fetched in pages and each page's rows cursor is fully drained
// and closed BEFORE fn runs: the store has a single SQLite connection, so a
// cursor held open across a write inside fn would self-deadlock.
func (s *Store) EachRawBatch(ctx context.Context, fn func(project string, payload []byte) error) error {
	const pageSize = 100
	lastID := int64(0)
	for {
		batches, maxID, err := s.rawBatchPage(ctx, lastID, pageSize)
		if err != nil {
			return err
		}
		if len(batches) == 0 {
			return nil
		}
		for _, b := range batches {
			if err := fn(b.project, b.payload); err != nil {
				return err
			}
		}
		lastID = maxID
	}
}

type rawBatch struct {
	project string
	payload []byte
}

func (s *Store) rawBatchPage(ctx context.Context, afterID int64, limit int) ([]rawBatch, int64, error) {
	rows, err := s.reader.QueryContext(ctx,
		`SELECT id, project, payload FROM raw_batches WHERE id > ? ORDER BY id LIMIT ?`, afterID, limit)
	if err != nil {
		return nil, 0, err
	}
	defer rows.Close()

	var batches []rawBatch
	var maxID int64
	for rows.Next() {
		var id int64
		var b rawBatch
		if err := rows.Scan(&id, &b.project, &b.payload); err != nil {
			return nil, 0, err
		}
		batches = append(batches, b)
		maxID = id
	}
	return batches, maxID, rows.Err()
}
