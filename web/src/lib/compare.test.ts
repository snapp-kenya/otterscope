import { describe, expect, it } from "bun:test";
import type { Stats } from "@/api";
import {
  assertionNames,
  emptyCompareSide,
  parseCompareSearch,
  rateFor,
  searchFromSides,
  sideFromSearch,
  sideQuery,
} from "@/lib/compare";

const NOW = Date.parse("2026-08-21T12:00:00.000Z");

const stats = (assertionRates: Stats["assertionRates"]): Stats => ({
  runs: 2,
  errors: 1,
  avgDurationMs: 1200,
  p50DurationMs: 1000,
  p95DurationMs: 2000,
  totalCostUsd: 0.5,
  avgCostUsd: 0.25,
  inputTokens: 100,
  outputTokens: 50,
  assertionRates,
});

describe("sideQuery", () => {
  it("sends only the fields that are set", () => {
    expect(sideQuery(emptyCompareSide, NOW)).toBe("");
    const query = new URLSearchParams(
      sideQuery(
        {
          status: "error",
          model: "claude",
          service: "svc",
          prompt: "p1",
          range: "",
        },
        NOW,
      ),
    );
    expect(query.get("status")).toBe("error");
    expect(query.get("model")).toBe("claude");
    expect(query.get("service")).toBe("svc");
    expect(query.get("prompt")).toBe("p1");
    expect(query.has("since")).toBe(false);
  });

  it("turns a range into the same since bound the run list uses", () => {
    const query = new URLSearchParams(
      sideQuery({ ...emptyCompareSide, range: "24h" }, NOW),
    );
    expect(query.get("since")).toBe("2026-08-20T12:00:00.000Z");
  });

});

describe("compare search mapping", () => {
  it("round-trips both sides through the URL search object", () => {
    const a = {
      status: "error",
      model: "claude",
      service: "",
      prompt: "p1",
      range: "7d",
    };
    const b = {
      status: "",
      model: "gpt",
      service: "svc",
      prompt: "",
      range: "24h",
    };
    const search = searchFromSides(a, b);
    expect(search).toEqual({
      a_status: "error",
      a_model: "claude",
      a_prompt: "p1",
      a_range: "7d",
      b_model: "gpt",
      b_service: "svc",
      b_range: "24h",
    });
    const parsed = parseCompareSearch(search as Record<string, unknown>);
    expect(sideFromSearch(parsed, "a")).toEqual(a);
    expect(sideFromSearch(parsed, "b")).toEqual(b);
  });

  it("keeps the old a_model/b_model URL shape", () => {
    const parsed = parseCompareSearch({
      a_model: "claude-sonnet-5",
      b_model: "gpt-5.4-mini",
    });
    expect(sideFromSearch(parsed, "a").model).toBe("claude-sonnet-5");
    expect(sideFromSearch(parsed, "b").model).toBe("gpt-5.4-mini");
  });

  it("drops empty and non-string values", () => {
    expect(parseCompareSearch({ a_model: "", b_model: 7 })).toEqual({});
  });
});

describe("assertion rates", () => {
  const sideA = stats([
    { assertionId: 1, name: "polite", passed: 2, total: 2 },
    { assertionId: 2, name: "json", passed: 0, total: 2 },
  ]);
  const sideB = stats([
    { assertionId: 2, name: "json", passed: 1, total: 4 },
  ]);

  it("unions assertion names across both sides once", () => {
    expect(assertionNames([sideA, sideB])).toEqual([
      [1, "polite"],
      [2, "json"],
    ]);
  });

  it("finds a side's rate and misses an assertion it did not see", () => {
    expect(rateFor(sideA, 2)).toEqual({
      assertionId: 2,
      name: "json",
      passed: 0,
      total: 2,
    });
    expect(rateFor(sideB, 1)).toBeUndefined();
    expect(rateFor(undefined, 1)).toBeUndefined();
  });
});
