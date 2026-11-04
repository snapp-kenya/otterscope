import { useNavigate, useSearch } from "@tanstack/react-router";
import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { errorMessage, type Stats } from "@/api";
import {
  assertionNames,
  COMPARE_DEBOUNCE_MS,
  rateFor,
  searchFromSides,
  sideFromSearch,
  type CompareSide,
} from "@/lib/compare";
import { fmtCost, fmtDuration, fmtPct, fmtTokens } from "@/lib/format";
import { statsQuery } from "@/lib/queries";
import { useDebouncedValue } from "@/lib/use-debounced";
import { CompareSideForm } from "@/components/compare-side-form";
import { ErrorNotice } from "@/components/error-notice";
import { PageHeader } from "@/components/page-header";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

type Metric = { label: string; value: (stats: Stats) => string };

const metrics: Metric[] = [
  { label: "runs", value: (stats) => String(stats.runs) },
  { label: "error rate", value: (stats) => fmtPct(stats.errors, stats.runs) },
  { label: "p50 duration", value: (stats) => fmtDuration(stats.p50DurationMs) },
  { label: "p95 duration", value: (stats) => fmtDuration(stats.p95DurationMs) },
  { label: "avg cost / run", value: (stats) => fmtCost(stats.avgCostUsd) },
  { label: "total cost", value: (stats) => fmtCost(stats.totalCostUsd) },
  {
    label: "tokens in / out",
    value: (stats) =>
      `${fmtTokens(stats.inputTokens)} / ${fmtTokens(stats.outputTokens)}`,
  },
];

const cellValue = (
  result: UseQueryResult<Stats>,
  read: (stats: Stats) => string,
): string => (result.data ? read(result.data) : "…");

export const ComparePage = () => {
  const search = useSearch({ from: "/compare" });
  const navigate = useNavigate();
  const sideA = sideFromSearch(search, "a");
  const sideB = sideFromSearch(search, "b");
  const queriedA = useDebouncedValue(sideA, COMPARE_DEBOUNCE_MS);
  const queriedB = useDebouncedValue(sideB, COMPARE_DEBOUNCE_MS);
  const statsA = useQuery(statsQuery(queriedA));
  const statsB = useQuery(statsQuery(queriedB));

  const updateSide = (prefix: "a" | "b", next: CompareSide) => {
    const nextA = prefix === "a" ? next : sideA;
    const nextB = prefix === "b" ? next : sideB;
    void navigate({
      to: "/compare",
      search: searchFromSides(nextA, nextB),
      replace: true,
    });
  };

  const assertions = assertionNames([statsA.data, statsB.data]);

  return (
    <div>
      <PageHeader
        title="Compare"
        description="Two filtered windows of runs, side by side."
      />
      <div className="compare-grid mt-4 grid gap-6 md:grid-cols-2">
        <div>
          <h3 className="text-xs text-muted-foreground">Side A</h3>
          <CompareSideForm
            side={sideA}
            onChange={(next) => updateSide("a", next)}
          />
          {statsA.isError ? (
            <ErrorNotice onRetry={() => void statsA.refetch()}>
              Side A — {errorMessage(statsA.error)}.
            </ErrorNotice>
          ) : null}
        </div>
        <div>
          <h3 className="text-xs text-muted-foreground">Side B</h3>
          <CompareSideForm
            side={sideB}
            onChange={(next) => updateSide("b", next)}
          />
          {statsB.isError ? (
            <ErrorNotice onRetry={() => void statsB.refetch()}>
              Side B — {errorMessage(statsB.error)}.
            </ErrorNotice>
          ) : null}
        </div>
      </div>

      <Table className="runs compare-table mt-6">
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead>metric</TableHead>
            <TableHead>A</TableHead>
            <TableHead>B</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {metrics.map((metric) => (
            <TableRow key={metric.label}>
              <TableCell className="text-muted-foreground">
                {metric.label}
              </TableCell>
              <TableCell className="numeric">
                {cellValue(statsA, metric.value)}
              </TableCell>
              <TableCell className="numeric">
                {cellValue(statsB, metric.value)}
              </TableCell>
            </TableRow>
          ))}
          {assertions.map(([id, name]) => (
            <TableRow key={`assertion-${id}`}>
              <TableCell className="text-muted-foreground">
                ✓ {name}
              </TableCell>
              <TableCell className="numeric">
                {cellValue(statsA, (stats) => {
                  const rate = rateFor(stats, id);
                  return rate ? fmtPct(rate.passed, rate.total) : "—";
                })}
              </TableCell>
              <TableCell className="numeric">
                {cellValue(statsB, (stats) => {
                  const rate = rateFor(stats, id);
                  return rate ? fmtPct(rate.passed, rate.total) : "—";
                })}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
};
