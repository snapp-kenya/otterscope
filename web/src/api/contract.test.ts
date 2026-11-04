import { describe, expect, it } from "bun:test";
import {
  decodeAlerts,
  decodeAuditEntries,
  decodeRunDetail,
  decodeRunList,
  decodeShares,
  decodeStats,
  decodeViews,
  endpoints,
} from "@/api/contract";

const run = {
  id: "abc",
  project: "default",
  service: "checkout",
  agentName: "planner",
  status: "ok" as const,
  start: "2026-11-04T09:00:00.000Z",
  durationMs: 1000,
  inputTokens: 10,
  outputTokens: 20,
  llmCalls: 1,
  toolCalls: 2,
  models: "claude",
  error: "",
};

describe("endpoint builders", () => {
  it("keeps the assumed API contract in one place", () => {
    expect(endpoints.runs("limit=2")).toBe("/api/runs?limit=2");
    expect(endpoints.runsCsv("q=x")).toBe("/api/runs.csv?q=x");
    expect(endpoints.run("abc")).toBe("/api/runs/abc");
    expect(endpoints.runShares("abc")).toBe("/api/runs/abc/shares");
    expect(endpoints.mintShare("abc")).toBe("/api/runs/abc/share");
    expect(endpoints.revokeShare("tok")).toBe("/api/shares/tok");
    expect(endpoints.sharedRun("tok")).toBe("/api/shared/tok");
    expect(endpoints.stats("model=gpt")).toBe("/api/stats?model=gpt");
    expect(endpoints.views()).toBe("/api/views");
    expect(endpoints.view(3)).toBe("/api/views/3");
    expect(endpoints.alerts()).toBe("/api/alerts");
    expect(endpoints.alert(3)).toBe("/api/alerts/3");
    expect(endpoints.audit(500)).toBe("/api/audit?limit=500");
    expect(endpoints.health()).toBe("/healthz");
  });

  it("encodes path segments", () => {
    expect(endpoints.run("a/b")).toBe("/api/runs/a%2Fb");
    expect(endpoints.sharedRun("a b")).toBe("/api/shared/a%20b");
  });
});

describe("wire decoders", () => {
  it("defaults optional arrays instead of leaking undefined", () => {
    const detail = decodeRunDetail({ run });
    expect(detail.steps).toEqual([]);
    expect(detail.assertionResults).toEqual([]);

    expect(decodeStats({
      runs: 1,
      errors: 0,
      avgDurationMs: 10,
      p50DurationMs: 10,
      p95DurationMs: 10,
      inputTokens: 1,
      outputTokens: 2,
    }).assertionRates).toEqual([]);

    expect(decodeRunList({})).toEqual([]);
    expect(decodeViews({})).toEqual([]);
    expect(decodeShares({})).toEqual([]);
    expect(decodeAlerts({})).toEqual([]);
    expect(decodeAuditEntries({})).toEqual([]);
  });

  it("passes through the fields the UI reads", () => {
    expect(decodeRunList({ runs: [run] })[0]?.service).toBe("checkout");
    expect(decodeRunDetail({ run }).run.id).toBe("abc");
  });
});
