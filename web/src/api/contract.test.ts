import { describe, expect, it } from "bun:test";
import {
  assertionTypes,
  decodeAlerts,
  decodeAssertions,
  decodeAuditEntries,
  decodeCreatedToken,
  decodeEvaluateReport,
  decodeRunDetail,
  decodeRunList,
  decodeShares,
  decodeSharedRun,
  decodeStats,
  decodeTokens,
  decodeViews,
  endpoints,
  evaluateQuery,
  runStatuses,
  stepKinds,
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

describe("contract enums", () => {
  it("matches the server's typed enums", () => {
    expect(runStatuses).toEqual(["running", "ok", "error"]);
    expect(stepKinds).toEqual(["agent", "llm", "tool", "generic"]);
    expect(assertionTypes).toContain("llm_judge");
    expect(assertionTypes).toContain("max_latency_ms");
  });
});

describe("endpoint builders", () => {
  it("keeps the Rust API contract in one place", () => {
    expect(endpoints.runs("limit=2")).toBe("/api/runs?limit=2");
    expect(endpoints.runsCsv("q=x")).toBe("/api/runs.csv?q=x");
    expect(endpoints.run("abc")).toBe("/api/runs/abc");
    expect(endpoints.runShares("abc")).toBe("/api/runs/abc/shares");
    expect(endpoints.mintShare("abc")).toBe("/api/runs/abc/share");
    expect(endpoints.revokeShare("tok")).toBe("/api/shares/tok");
    expect(endpoints.sharedRun("tok")).toBe("/api/shared/tok");
    expect(endpoints.stats("model=gpt")).toBe("/api/stats?model=gpt");
    expect(endpoints.assertions()).toBe("/api/assertions");
    expect(endpoints.assertions("prod")).toBe("/api/assertions?project=prod");
    expect(endpoints.assertion(3)).toBe("/api/assertions/3");
    expect(endpoints.evaluate("limit=2")).toBe(
      "/api/assertions/evaluate?limit=2",
    );
    expect(endpoints.views()).toBe("/api/views");
    expect(endpoints.view(3)).toBe("/api/views/3");
    expect(endpoints.alerts()).toBe("/api/alerts");
    expect(endpoints.alert(3)).toBe("/api/alerts/3");
    expect(endpoints.tokens()).toBe("/api/tokens");
    expect(endpoints.token("tok_1")).toBe("/api/tokens/tok_1");
    expect(endpoints.audit(500)).toBe("/api/audit?limit=500");
    expect(endpoints.health()).toBe("/healthz");
    expect(endpoints.metrics()).toBe("/metrics");
    expect(endpoints.mcp()).toBe("/mcp");
  });

  it("encodes path segments and query values", () => {
    expect(endpoints.run("a/b")).toBe("/api/runs/a%2Fb");
    expect(endpoints.sharedRun("a b")).toBe("/api/shared/a%20b");
    expect(endpoints.assertions("a b")).toBe(
      "/api/assertions?project=a%20b",
    );
    expect(endpoints.token("a/b")).toBe("/api/tokens/a%2Fb");
  });

  it("builds a bounded evaluate query", () => {
    expect(
      evaluateQuery({
        project: "prod",
        run: "abc",
        since: "2026-08-21T09:00:00Z",
        until: "2026-08-22T09:00:00Z",
        limit: 12,
      }),
    ).toBe(
      "project=prod&run=abc&since=2026-08-21T09%3A00%3A00Z&until=2026-08-22T09%3A00%3A00Z&limit=12",
    );
    expect(evaluateQuery({})).toBe("");
  });
});

describe("wire decoders", () => {
  it("decodes the array envelopes the Rust server always sends", () => {
    const detail = decodeRunDetail({ run, steps: [], assertionResults: [] });
    expect(detail.steps).toEqual([]);
    expect(detail.assertionResults).toEqual([]);

    expect(
      decodeStats({
        runs: 1,
        errors: 0,
        avgDurationMs: 10,
        p50DurationMs: 10,
        p95DurationMs: 10,
        inputTokens: 1,
        outputTokens: 2,
        assertionRates: [],
      }).assertionRates,
    ).toEqual([]);

    expect(decodeRunList({ runs: [] })).toEqual([]);
    expect(decodeViews({ views: [] })).toEqual([]);
    expect(decodeShares({ shares: [] })).toEqual([]);
    expect(decodeAlerts({ alerts: [] })).toEqual([]);
    expect(decodeAssertions({ assertions: [] })).toEqual([]);
    expect(decodeTokens({ tokens: [] })).toEqual([]);
    expect(decodeAuditEntries({ entries: [] })).toEqual([]);
  });

  it("turns a shared run into a run detail without assertion results", () => {
    expect(decodeSharedRun({ run, steps: [] })).toEqual({
      run,
      steps: [],
      assertionResults: [],
    });
  });

  it("passes through the fields the UI reads", () => {
    expect(decodeRunList({ runs: [run] })[0]?.service).toBe("checkout");
    expect(decodeRunDetail({ run, steps: [], assertionResults: [] }).run.id).toBe(
      "abc",
    );
  });

  it("decodes assertions, evaluation reports and tokens", () => {
    expect(
      decodeAssertions({
        assertions: [
          {
            id: 1,
            project: "default",
            name: "fast",
            type: "max_latency_ms",
            config: "20000",
            enabled: true,
          },
        ],
      })[0]?.type,
    ).toBe("max_latency_ms");

    expect(
      decodeEvaluateReport({
        runsEvaluated: 12,
        runsSkipped: 3,
        runLimit: 15,
        judgeCalls: 4,
        judgeCallsSkipped: 0,
        judgeCallLimit: 100,
      }),
    ).toEqual({
      runsEvaluated: 12,
      runsSkipped: 3,
      runLimit: 15,
      judgeCalls: 4,
      judgeCallsSkipped: 0,
      judgeCallLimit: 100,
    });

    expect(
      decodeTokens({
        tokens: [
          {
            id: "tok_1",
            name: "ci",
            scope: "read",
            createdAt: "2026-11-04T09:00:00Z",
          },
        ],
      })[0]?.scope,
    ).toBe("read");

    expect(
      decodeCreatedToken({
        id: "tok_1",
        token: "secret",
        name: "ci",
        scope: "admin",
        createdAt: "2026-11-04T09:00:00Z",
      }).token,
    ).toBe("secret");
  });
});
