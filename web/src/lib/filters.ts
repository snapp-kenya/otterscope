export type FilterState = {
  q: string;
  project: string;
  status: string;
  model: string;
  service: string;
  prompt: string;
  range: string;
};

export type RunsSearch = {
  q?: string;
  project?: string;
  status?: string;
  model?: string;
  service?: string;
  prompt?: string;
  range?: string;
  page?: number;
};

export const PAGE_SIZE = 100;

export const emptyFilters: FilterState = {
  q: "",
  project: "",
  status: "",
  model: "",
  service: "",
  prompt: "",
  range: "",
};

const RANGE_HOURS: Record<string, number> = { "1h": 1, "24h": 24, "7d": 168 };

const asText = (value: unknown): string =>
  typeof value === "string" ? value : "";

const asPage = (value: unknown): number | undefined => {
  if (value === undefined || value === null || value === "") return undefined;
  const page = typeof value === "number" ? value : Number(value);
  return Number.isInteger(page) && page > 0 ? page : undefined;
};

export const withFilterDefaults = (
  params: unknown,
): FilterState => {
  const out = { ...emptyFilters };
  if (typeof params !== "object" || params === null) return out;
  const record = params as Record<string, unknown>;
  for (const key of Object.keys(emptyFilters) as (keyof FilterState)[]) {
    const value = record[key];
    if (typeof value === "string") out[key] = value;
  }
  return out;
};

export const rangeSince = (
  range: string,
  nowMs: number = Date.now(),
): string | null => {
  const hours = RANGE_HOURS[range];
  if (!hours) return null;
  return new Date(nowMs - hours * 3600_000).toISOString();
};

export const filtersToQuery = (
  filters: FilterState,
  page = 0,
  limit = PAGE_SIZE,
  nowMs: number = Date.now(),
): string => {
  const query = new URLSearchParams();
  query.set("limit", String(limit));
  if (page > 0) query.set("offset", String(page * PAGE_SIZE));
  if (filters.q) query.set("q", filters.q);
  if (filters.project) query.set("project", filters.project);
  if (filters.status) query.set("status", filters.status);
  if (filters.model) query.set("model", filters.model);
  if (filters.service) query.set("service", filters.service);
  if (filters.prompt) query.set("prompt", filters.prompt);
  const since = rangeSince(filters.range, nowMs);
  if (since) query.set("since", since);
  return query.toString();
};

export const parseRunsSearch = (
  raw: Record<string, unknown>,
): RunsSearch => ({
  q: asText(raw.q) || undefined,
  project: asText(raw.project) || undefined,
  status: asText(raw.status) || undefined,
  model: asText(raw.model) || undefined,
  service: asText(raw.service) || undefined,
  prompt: asText(raw.prompt) || undefined,
  range: asText(raw.range) || undefined,
  page: asPage(raw.page),
});

export const searchFromFilters = (
  filters: FilterState,
  page = 0,
): RunsSearch => {
  const search: RunsSearch = {};
  if (filters.q) search.q = filters.q;
  if (filters.project) search.project = filters.project;
  if (filters.status) search.status = filters.status;
  if (filters.model) search.model = filters.model;
  if (filters.service) search.service = filters.service;
  if (filters.prompt) search.prompt = filters.prompt;
  if (filters.range) search.range = filters.range;
  if (page > 0) search.page = page;
  return search;
};

export const filtersFromSearch = (search: RunsSearch): FilterState => ({
  q: search.q ?? "",
  project: search.project ?? "",
  status: search.status ?? "",
  model: search.model ?? "",
  service: search.service ?? "",
  prompt: search.prompt ?? "",
  range: search.range ?? "",
});

export const hasActiveFilters = (filters: FilterState): boolean =>
  Boolean(
    filters.q ||
      filters.project ||
      filters.status ||
      filters.model ||
      filters.service ||
      filters.prompt ||
      filters.range,
  );
