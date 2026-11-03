// Filter state and its URL <-> query-string mapping. Kept out of the
// component file so it can be unit-tested without rendering, and so
// Filters.tsx exports only a component (react-refresh keeps working).
import { withBase } from "./base";

export type FilterState = {
  q: string;
  project: string;
  status: string;
  model: string;
  service: string;
  prompt: string;
  range: string; // "", "1h", "24h", "7d"
};

// One page of the run list. The server caps `limit` at 500 and pages with
// `offset`; the list asks for one row past the page so it can tell "exactly a
// page" from "there is more" without a count endpoint.
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

// Saved views come back from the server as whatever was stored when they were
// saved, so a view created before a filter existed has no key for it. Applying
// that object straight into state leaves the missing keys undefined; a
// controlled input whose value is undefined is treated as uncontrolled by
// React and then ignores later views too. Complete every applied view over the
// full default set — and drop nulls or the wrong type the same way.
export function withFilterDefaults(
  params: Partial<FilterState> | null | undefined,
): FilterState {
  const out = { ...emptyFilters };
  if (!params) return out;
  for (const k of Object.keys(emptyFilters) as (keyof FilterState)[]) {
    const v = params[k];
    if (typeof v === "string") out[k] = v;
  }
  return out;
}

export function filtersFromURL(): FilterState {
  const q = new URLSearchParams(window.location.search);
  return {
    q: q.get("q") ?? "",
    project: q.get("project") ?? "",
    status: q.get("status") ?? "",
    model: q.get("model") ?? "",
    service: q.get("service") ?? "",
    prompt: q.get("prompt") ?? "",
    range: q.get("range") ?? "",
  };
}

// `page` is 0-based; `limit` is a page plus the one lookahead row the list
// uses to detect a next page (or exactly PAGE_SIZE for the CSV path, which
// ignores it and applies its own cap server-side).
export function filtersToQuery(
  f: FilterState,
  page = 0,
  limit = PAGE_SIZE,
): string {
  const q = new URLSearchParams();
  q.set("limit", String(limit));
  if (page > 0) q.set("offset", String(page * PAGE_SIZE));
  if (f.q) q.set("q", f.q);
  if (f.project) q.set("project", f.project);
  if (f.status) q.set("status", f.status);
  if (f.model) q.set("model", f.model);
  if (f.service) q.set("service", f.service);
  if (f.prompt) q.set("prompt", f.prompt);
  if (f.range) {
    const hours = { "1h": 1, "24h": 24, "7d": 168 }[f.range];
    if (hours) {
      q.set("since", new Date(Date.now() - hours * 3600_000).toISOString());
    }
  }
  return q.toString();
}

export function syncURL(f: FilterState) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(f)) if (v) q.set(k, v);
  const qs = q.toString();
  window.history.replaceState(null, "", withBase("/") + (qs ? `?${qs}` : ""));
}
