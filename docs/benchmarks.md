# Benchmarks

> **Go-era reference numbers.** The measurements below were taken on the
> original Go implementation. There is no Rust benchmark harness yet, so they
> do not describe the Rust build and must not be quoted as current
> performance. The shape of the trade-offs still informs sizing; the absolute
> figures do not.

Otterscope is a single self-contained binary over one SQLite file. These
benchmarks map where that design comfortably sits and where it eventually
strains, so you can size an instance honestly.

## Running them

The original harness lives in `internal/store/bench_test.go` in the retained
Go reference tree:

```sh
go test -run=^$ -bench=. -benchmem ./internal/store/
```

`BenchmarkIngest` measures the write path (`UpsertSteps`: insert + run
re-derivation + FTS index). The query benchmarks (`ListRuns`, `GetStats`,
`Search`) each seed a database of N runs (3 steps/run) and measure one query's
latency at that size.

## Results

Go-era results, measured before the Rust port:

| Operation | Dataset | Result |
| --- | --- | --- |
| **Ingest** (`UpsertSteps`, one run = 3 spans) | — | **~3,000 spans/s** on the single-run write path |
| **ListRuns** (page of 50, newest-first) | 1,000 runs | **~0.17 ms** |
| **ListRuns** (page of 50, newest-first) | 10,000 runs | **~0.17 ms** (flat — indexed) |
| **GetStats** (count, error rate, p50/p95, cost, assertion rates) | 1,000 runs | **~0.9 ms** |
| **Full-text search** (`ListRuns` with a query) | 1,000 runs | **~41 ms** when the term matches nearly every run |

Measured on an AMD Ryzen 7 9700X (WSL2). `spans/s` is the serialized
single-run write path; real OTLP ingest batches many runs per request, so
effective throughput is higher. `ListRuns` latency is essentially flat from
1k to 10k because runs are indexed by `start_ns`/status/service/project.
Full-text search cost scales with the *number of matching runs* — the ~41 ms
figure is a worst case where the search term appears in almost every run; a
selective term returns far faster.

_(Measured on the development machine; your numbers will vary with disk and
CPU. The shape — not the absolute figures — is the point.)_

## Refreshing them for the Rust build

There is no `cargo bench` target yet. To make these numbers current:

1. Port the four benchmarks (`BenchmarkIngest`, `BenchmarkListRuns`,
   `BenchmarkGetStats`, `BenchmarkSearch`) to a Rust bench target that opens
   `store::Store` on a temporary database.
2. Seed the same 1,000- and 10,000-run datasets (3 steps per run) and the same
   full-text queries, so the before/after comparison is like for like.
3. Run on a named machine and record CPU, disk, commit and Rust version with
   the results.
4. Replace the table and drop the Go-era banner only when the Rust figures are
   committed; until then the README must not quote a throughput number.

## Reading them

These conclusions are structural and still expected to hold, but re-measure
them on the Rust build before using them for capacity planning.

- **Ingest** is a serialized single-writer path (by design — it avoids
  SQLITE_BUSY churn). The number to watch is spans/sec; a few thousand
  spans/sec is comfortably ahead of a small-team agent workload.
- **Queries** stay indexed (runs by `start_ns`, status, service, project), so
  list/stats latency grows sub-linearly with run count. Full-text search uses
  FTS5.
- **When you outgrow it:** if ingest throughput or query latency becomes the
  bottleneck at your volume, that's the signal to shard by project across
  instances or revisit the storage layer (deferred in ADR-0002). For an
  individual or small team, a single instance has substantial headroom.
