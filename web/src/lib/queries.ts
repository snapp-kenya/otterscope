import { keepPreviousData, queryOptions } from "@tanstack/react-query";
import {
  fetchAlerts,
  fetchAudit,
  fetchHealth,
  fetchRun,
  fetchRuns,
  fetchSharedRun,
  fetchShares,
  fetchStats,
  fetchViews,
} from "@/api";
import { sideQuery, type CompareSide } from "@/lib/compare";
import { filtersToQuery, PAGE_SIZE, type FilterState } from "@/lib/filters";

export const queryKeys = {
  runsRoot: ["runs"] as const,
  runRoot: ["run"] as const,
  runs: (filters: FilterState, page: number) => ["runs", filters, page] as const,
  run: (id: string) => ["run", id] as const,
  shared: (token: string) => ["shared", token] as const,
  views: () => ["views"] as const,
  shares: (id: string) => ["shares", id] as const,
  stats: (side: CompareSide) => ["stats", side] as const,
  alerts: () => ["alerts"] as const,
  audit: () => ["audit"] as const,
  health: () => ["health"] as const,
};

export const runsQuery = (filters: FilterState, page: number) =>
  queryOptions({
    queryKey: queryKeys.runs(filters, page),
    queryFn: ({ signal }) =>
      fetchRuns(
        filtersToQuery(filters, page, PAGE_SIZE + 1),
        signal,
      ),
  });

export const runQuery = (id: string) =>
  queryOptions({
    queryKey: queryKeys.run(id),
    queryFn: ({ signal }) => fetchRun(id, signal),
    retry: false,
    refetchInterval: 2000,
  });

export const sharedRunQuery = (token: string) =>
  queryOptions({
    queryKey: queryKeys.shared(token),
    queryFn: ({ signal }) => fetchSharedRun(token, signal),
    retry: false,
  });

export const viewsQuery = () =>
  queryOptions({
    queryKey: queryKeys.views(),
    queryFn: ({ signal }) => fetchViews(signal),
  });

export const sharesQuery = (id: string) =>
  queryOptions({
    queryKey: queryKeys.shares(id),
    queryFn: ({ signal }) => fetchShares(id, signal),
  });

export const statsQuery = (side: CompareSide) =>
  queryOptions({
    queryKey: queryKeys.stats(side),
    queryFn: ({ signal }) => fetchStats(sideQuery(side), signal),
    placeholderData: keepPreviousData,
    retry: false,
  });

export const alertsQuery = () =>
  queryOptions({
    queryKey: queryKeys.alerts(),
    queryFn: ({ signal }) => fetchAlerts(signal),
    refetchInterval: 5000,
  });

export const auditQuery = () =>
  queryOptions({
    queryKey: queryKeys.audit(),
    queryFn: ({ signal }) => fetchAudit(500, signal),
  });

export const healthQuery = () =>
  queryOptions({
    queryKey: queryKeys.health(),
    queryFn: ({ signal }) => fetchHealth(signal),
    retry: false,
    staleTime: Infinity,
  });
