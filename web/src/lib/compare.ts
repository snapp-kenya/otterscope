import type { AssertionRate, Stats } from "@/api";
import { rangeSince } from "@/lib/filters";

export type CompareSide = {
  status: string;
  model: string;
  service: string;
  prompt: string;
  range: string;
};

export type ComparePrefix = "a" | "b";

export type CompareSearch = {
  a_status?: string;
  a_model?: string;
  a_service?: string;
  a_prompt?: string;
  a_range?: string;
  b_status?: string;
  b_model?: string;
  b_service?: string;
  b_prompt?: string;
  b_range?: string;
};

export const COMPARE_DEBOUNCE_MS = 250;

export const emptyCompareSide: CompareSide = {
  status: "",
  model: "",
  service: "",
  prompt: "",
  range: "",
};

const sideKeys = ["status", "model", "service", "prompt", "range"] as const;

type SideKey = (typeof sideKeys)[number];

export const compareSearchKey = (
  prefix: ComparePrefix,
  key: SideKey,
): keyof CompareSearch => `${prefix}_${key}`;

export const sideFromSearch = (
  search: CompareSearch,
  prefix: ComparePrefix,
): CompareSide => {
  const side = { ...emptyCompareSide };
  for (const key of sideKeys) {
    const value = search[compareSearchKey(prefix, key)];
    if (typeof value === "string") side[key] = value;
  }
  return side;
};

export const sideQuery = (
  side: CompareSide,
  nowMs: number = Date.now(),
): string => {
  const query = new URLSearchParams();
  if (side.status) query.set("status", side.status);
  if (side.model) query.set("model", side.model);
  if (side.service) query.set("service", side.service);
  if (side.prompt) query.set("prompt", side.prompt);
  const since = rangeSince(side.range, nowMs);
  if (since) query.set("since", since);
  return query.toString();
};

export const searchFromSides = (
  a: CompareSide,
  b: CompareSide,
): CompareSearch => {
  const search: CompareSearch = {};
  for (const key of sideKeys) {
    if (a[key]) search[compareSearchKey("a", key)] = a[key];
    if (b[key]) search[compareSearchKey("b", key)] = b[key];
  }
  return search;
};

export const parseCompareSearch = (
  raw: Record<string, unknown>,
): CompareSearch => {
  const search: CompareSearch = {};
  for (const prefix of ["a", "b"] as const) {
    for (const key of sideKeys) {
      const value = raw[compareSearchKey(prefix, key)];
      if (typeof value === "string" && value !== "") {
        search[compareSearchKey(prefix, key)] = value;
      }
    }
  }
  return search;
};

export const assertionNames = (
  sides: (Stats | undefined)[],
): [number, string][] => {
  const names = new Map<number, string>();
  for (const stats of sides) {
    for (const rate of stats?.assertionRates ?? []) {
      names.set(rate.assertionId, rate.name);
    }
  }
  return [...names.entries()];
};

export const rateFor = (
  stats: Stats | undefined,
  assertionId: number,
): AssertionRate | undefined =>
  stats?.assertionRates.find((rate) => rate.assertionId === assertionId);
