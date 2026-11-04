import { useState } from "react";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useMutation, useQuery } from "@tanstack/react-query";
import { errorMessage } from "@/api";
import { exportRunsCsv } from "@/lib/csv";
import { BASE_PATH } from "@/lib/base";
import {
  filtersFromSearch,
  hasActiveFilters,
  PAGE_SIZE,
  searchFromFilters,
  type FilterState,
} from "@/lib/filters";
import { fmtCost, fmtDuration, fmtStart, fmtTokens } from "@/lib/format";
import { runsQuery } from "@/lib/queries";
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/status-badge";
import { RunsFilters } from "@/components/runs-filters";
import { SavedViews } from "@/components/saved-views";
import { PageHeader } from "@/components/page-header";
import { ErrorNotice, LoadingHint } from "@/components/error-notice";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export const RunsPage = () => {
  const search = useSearch({ from: "/" });
  const navigate = useNavigate();
  const filters = filtersFromSearch(search);
  const page = search.page ?? 0;
  const runsResult = useQuery(runsQuery(filters, page));
  // The server's single listener is on this origin; keep the hint honest for
  // whatever host and base path the deployment publishes.
  const otlpEndpoint = `${window.location.origin}${BASE_PATH}`;
  const [exportNotice, setExportNotice] = useState<string | null>(null);
  const exportMutation = useMutation({
    mutationFn: () => exportRunsCsv(filters),
    onSuccess: (notice) => setExportNotice(notice),
    onError: (error) =>
      setExportNotice(`Export failed — ${errorMessage(error)}.`),
  });

  const applyFilters = (next: FilterState) => {
    setExportNotice(null);
    void navigate({
      to: "/",
      search: searchFromFilters(next),
      replace: true,
    });
  };

  const goToPage = (next: number) => {
    void navigate({
      to: "/",
      search: searchFromFilters(filters, next),
      replace: true,
    });
  };

  const openRun = (id: string) => {
    void navigate({ to: "/runs/$runId", params: { runId: id } });
  };

  const rows = (runsResult.data ?? []).slice(0, PAGE_SIZE);
  const hasMore = (runsResult.data?.length ?? 0) > PAGE_SIZE;
  const hasFilters = hasActiveFilters(filters);
  const from = page * PAGE_SIZE + 1;
  const to = page * PAGE_SIZE + rows.length;

  return (
    <>
      <PageHeader
        title="Runs"
        description="Live agent runs, newest first."
      />
      <SavedViews current={filters} onApply={applyFilters} />
      <div className="mt-4 flex flex-wrap items-start gap-2">
        <RunsFilters filters={filters} onChange={applyFilters} />
        <Button
          variant="outline"
          size="sm"
          className="mt-px whitespace-nowrap"
          title="Export filtered runs as CSV"
          onClick={() => exportMutation.mutate()}
          disabled={exportMutation.isPending}
        >
          export CSV
        </Button>
      </div>
      {exportNotice ? (
        <p className="mt-1 text-xs text-danger" role="status">
          {exportNotice}
        </p>
      ) : null}

      {runsResult.isError ? (
        <ErrorNotice onRetry={() => void runsResult.refetch()}>
          {runsResult.data === undefined
            ? "Couldn't load runs — "
            : "Couldn't refresh — "}
          {errorMessage(runsResult.error)}.
        </ErrorNotice>
      ) : null}

      {runsResult.isPending ? <LoadingHint /> : null}

      {runsResult.isSuccess && rows.length === 0 ? (
        <div className="empty mt-12 text-center">
          <p className="text-sm">
            {hasFilters ? "No runs match these filters." : "No runs yet."}
          </p>
          {hasFilters ? (
            <p className="mt-1 text-xs text-muted-foreground">
              Clear the search and filters to see everything again.
            </p>
          ) : (
            <p className="mt-1 text-xs text-muted-foreground">
              Point your agent&apos;s OpenTelemetry exporter here:
              <br />
              <code className="mt-2 inline-block rounded-md bg-surface-raised px-2.5 py-1.5 text-[0.8rem]">
                export OTEL_EXPORTER_OTLP_ENDPOINT={otlpEndpoint}
              </code>
            </p>
          )}
        </div>
      ) : null}

      {rows.length > 0 ? (
        <>
          <div className="list-pager mt-3 flex flex-wrap items-baseline gap-3 text-xs text-muted-foreground">
            <span>
              showing {from}–{to}
              {hasMore ? " · more match" : " · end of the list"}
            </span>
            <Button
              variant="link"
              size="sm"
              disabled={page === 0}
              onClick={() => goToPage(page - 1)}
            >
              ← newer
            </Button>
            <Button
              variant="link"
              size="sm"
              disabled={!hasMore}
              onClick={() => goToPage(page + 1)}
            >
              older →
            </Button>
          </div>
          <Table className="runs mt-4">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead>status</TableHead>
                <TableHead>started</TableHead>
                <TableHead>duration</TableHead>
                <TableHead>service / agent</TableHead>
                <TableHead>models</TableHead>
                <TableHead className="text-right">cost</TableHead>
                <TableHead className="text-right">tokens in</TableHead>
                <TableHead className="text-right">out</TableHead>
                <TableHead className="text-right">llm</TableHead>
                <TableHead className="text-right">tools</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((run) => (
                <TableRow
                  key={run.id}
                  className="clickable cursor-pointer hover:bg-surface-raised/60"
                  title={run.error || run.id}
                  tabIndex={0}
                  onClick={() => openRun(run.id)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") openRun(run.id);
                  }}
                >
                  <TableCell>
                    <StatusBadge status={run.status} />
                  </TableCell>
                  <TableCell className="numeric text-muted-foreground">
                    {fmtStart(run.start)}
                  </TableCell>
                  <TableCell className="numeric text-muted-foreground">
                    {fmtDuration(run.durationMs)}
                  </TableCell>
                  <TableCell>
                    {run.service || "—"}
                    {run.agentName ? (
                      <span className="text-muted-foreground">
                        {" "}
                        / {run.agentName}
                      </span>
                    ) : null}
                  </TableCell>
                  <TableCell
                    className="max-w-[14rem] truncate"
                    title={run.models}
                  >
                    {run.models || "—"}
                  </TableCell>
                  <TableCell className="numeric text-right">
                    {fmtCost(run.costUsd, run.costPartial)}
                  </TableCell>
                  <TableCell className="numeric text-right">
                    {fmtTokens(run.inputTokens)}
                  </TableCell>
                  <TableCell className="numeric text-right">
                    {fmtTokens(run.outputTokens)}
                  </TableCell>
                  <TableCell className="numeric text-right">
                    {run.llmCalls}
                  </TableCell>
                  <TableCell className="numeric text-right">
                    {run.toolCalls}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </>
      ) : null}
    </>
  );
};
