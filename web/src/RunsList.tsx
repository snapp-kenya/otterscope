import { useEffect, useState } from "react";
import {
  fmtCost,
  fmtDuration,
  fmtStart,
  fmtTokens,
  type Run,
  apiFetch,
  csvTruncationNotice,
  failureMessage,
  transportMessage,
} from "./api";
import { withBase } from "./base";
import Filters from "./Filters";
import {
  filtersFromURL,
  filtersToQuery,
  syncURL,
  PAGE_SIZE,
  type FilterState,
} from "./filterState";
import SavedViews from "./SavedViews";
import { useLiveTick } from "./live";

export default function RunsList({
  navigate,
}: {
  navigate: (path: string) => void;
}) {
  const [runs, setRuns] = useState<Run[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [filters, setFilters] = useState<FilterState>(filtersFromURL);
  const [exportNotice, setExportNotice] = useState<string | null>(null);
  const tick = useLiveTick();

  // RunsList owns the filter state (so saved views can drive it); keep the
  // URL in sync whenever it changes.
  useEffect(() => {
    syncURL(filters);
  }, [filters]);

  useEffect(() => {
    let stop = false;
    // Ask for one row past the page: if it comes back, there is another page.
    // The server pages with offset and caps limit at 500.
    const url = withBase(
      `/api/runs?${filtersToQuery(filters, page, PAGE_SIZE + 1)}`,
    );
    const load = async () => {
      try {
        const res = await apiFetch(url);
        if (!res.ok) {
          // A 500, an offline network or a token prompt the reader dismissed
          // is a settled failure. Left as the null sentinel it renders
          // "loading…" forever, which is a lie about what happened.
          const msg = await failureMessage(res);
          if (!stop) setError(msg);
          return;
        }
        const data = await res.json();
        if (stop) return;
        const rows: Run[] = data.runs ?? [];
        // The lookahead row is evidence of a next page, not something to show.
        setRuns(rows.slice(0, PAGE_SIZE));
        setHasMore(rows.length > PAGE_SIZE);
        setError(null);
      } catch (err) {
        if (!stop) setError(transportMessage(err));
      }
    };
    load();
    return () => {
      stop = true;
    };
  }, [filters, page, attempt, tick]);

  // A different filter set or page is a different question; showing the old
  // answer while the new one loads would label other runs with this page's
  // range.
  const queryFilters = (next: FilterState) => {
    setFilters(next);
    setPage(0);
    setRuns(null);
    setError(null);
  };

  const goToPage = (next: number) => {
    setPage(next);
    setRuns(null);
    setError(null);
  };

  const retry = () => {
    setError(null);
    setAttempt((n) => n + 1);
  };

  const exportCsv = async () => {
    setExportNotice(null);
    let res: Response;
    try {
      res = await apiFetch(withBase(`/api/runs.csv?${filtersToQuery(filters)}`));
    } catch (err) {
      setExportNotice(`Export failed — ${transportMessage(err)}.`);
      return;
    }
    if (!res.ok) {
      setExportNotice("Export failed.");
      return;
    }
    setExportNotice(csvTruncationNotice(res));
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "otterscope-runs.csv";
    a.click();
    URL.revokeObjectURL(url);
  };

  // The text search and the prompt filter are filters too. Leaving them out
  // meant a search with no hits announced "No runs yet." and offered the
  // onboarding copy — wrong advice for someone who mistyped a term.
  const hasFilters = Boolean(
    filters.q ||
      filters.project ||
      filters.status ||
      filters.model ||
      filters.service ||
      filters.prompt ||
      filters.range,
  );

  const from = page * PAGE_SIZE + 1;
  const to = page * PAGE_SIZE + (runs?.length ?? 0);

  return (
    <>
      <SavedViews current={filters} onApply={queryFilters} />
      <div className="list-toolbar">
        <Filters value={filters} onChange={queryFilters} />
        <button className="csv-btn" onClick={exportCsv} title="Export filtered runs as CSV">
          export CSV
        </button>
      </div>
      {exportNotice && (
        <p className="export-notice" role="status">
          {exportNotice}
        </p>
      )}
      {error && (
        <p className="load-error" role="status">
          <span className="run-error">
            {runs === null ? "Couldn't load runs — " : "Couldn't refresh — "}
            {error}.
          </span>
          <button className="link-btn accent" onClick={retry}>
            retry
          </button>
        </p>
      )}
      {runs === null && !error && <p className="hint">loading…</p>}
      {runs !== null && runs.length === 0 && (
        <div className="empty">
          <p>{hasFilters ? "No runs match these filters." : "No runs yet."}</p>
          {hasFilters ? (
            <p className="hint">
              Clear the search and filters to see everything again.
            </p>
          ) : (
            <p className="hint">
              Point your agent&apos;s OpenTelemetry exporter here:
              <br />
              <code>
                export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
              </code>
            </p>
          )}
        </div>
      )}
      {runs !== null && runs.length > 0 && (
      <>
      <p className="list-pager">
        <span>
          showing {from}–{to}
          {hasMore ? " · more match" : " · end of the list"}
        </span>
        <button
          className="link-btn accent"
          disabled={page === 0}
          onClick={() => goToPage(page - 1)}
        >
          ← newer
        </button>
        <button
          className="link-btn accent"
          disabled={!hasMore}
          onClick={() => goToPage(page + 1)}
        >
          older →
        </button>
      </p>
      <table className="runs">
      <thead>
        <tr>
          <th>status</th>
          <th>started</th>
          <th>duration</th>
          <th>service / agent</th>
          <th>models</th>
          <th className="num">cost</th>
          <th className="num">tokens in</th>
          <th className="num">out</th>
          <th className="num">llm</th>
          <th className="num">tools</th>
        </tr>
      </thead>
      <tbody>
        {runs.map((r) => (
          <tr
            key={r.id}
            className="clickable"
            title={r.error || r.id}
            onClick={() => navigate(`/runs/${r.id}`)}
          >
            <td>
              <span className={`badge ${r.status}`}>{r.status}</span>
            </td>
            <td>{fmtStart(r.start)}</td>
            <td>{fmtDuration(r.durationMs)}</td>
            <td>
              {r.service || "—"}
              {r.agentName && <span className="agent"> / {r.agentName}</span>}
            </td>
            <td>{r.models || "—"}</td>
            <td className="num">{fmtCost(r.costUsd, r.costPartial)}</td>
            <td className="num">{fmtTokens(r.inputTokens)}</td>
            <td className="num">{fmtTokens(r.outputTokens)}</td>
            <td className="num">{r.llmCalls}</td>
            <td className="num">{r.toolCalls}</td>
          </tr>
        ))}
      </tbody>
      </table>
      </>
      )}
    </>
  );
}
