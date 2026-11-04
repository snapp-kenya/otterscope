import { afterEach, describe, expect, it } from "bun:test";
import {
  ApiError,
  createAlert,
  createToken,
  evaluateAssertions,
  fetchMetricsText,
  fetchRuns,
  fetchSharedRun,
  fetchTokens,
  type AlertInput,
} from "@/api";

type FetchCall = { url: string; init: RequestInit };

const originalFetch = globalThis.fetch;

const stubFetch = (responses: Response[]): FetchCall[] => {
  const calls: FetchCall[] = [];
  globalThis.fetch = (async (
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> => {
    calls.push({ url: String(input), init: init ?? {} });
    return responses.shift() ?? new Response("{}");
  }) as typeof fetch;
  return calls;
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const run = {
  id: "abc",
  project: "default",
  service: "checkout",
  agentName: "planner",
  status: "ok",
  start: "2026-11-04T09:00:00.000Z",
  durationMs: 1000,
  inputTokens: 10,
  outputTokens: 20,
  llmCalls: 1,
  toolCalls: 2,
  models: "claude",
  error: "",
};

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("Rust API request shapes", () => {
  it("fetches and decodes a runs envelope", async () => {
    const calls = stubFetch([json({ runs: [run] })]);
    const runs = await fetchRuns("limit=2&status=ok");
    expect(calls[0]!.url).toBe("/api/runs?limit=2&status=ok");
    expect(runs[0]?.service).toBe("checkout");
  });

  it("surfaces the typed-enum 400 as an ApiError", async () => {
    stubFetch([
      json(
        { error: 'unknown status "bogus" (expected running, ok or error)' },
        400,
      ),
    ]);
    try {
      await fetchRuns("status=bogus");
      throw new Error("expected the request to fail");
    } catch (error) {
      expect(error).toBeInstanceOf(ApiError);
      if (error instanceof ApiError) {
        expect(error.status).toBe(400);
        expect(error.message).toContain("unknown status");
      }
    }
  });

  it("decodes a shared run without assertion results", async () => {
    stubFetch([json({ run, steps: [] })]);
    const detail = await fetchSharedRun("tok");
    expect(detail.assertionResults).toEqual([]);
  });

  it("posts a JSON alert body", async () => {
    const input: AlertInput = {
      name: "errors",
      type: "error_rate",
      threshold: 0.5,
      config: "",
      windowSecs: 3600,
      settleSecs: 300,
      webhookUrl: "https://hooks.example.test/hook",
    };
    const calls = stubFetch([
      json({ ...input, id: 1, project: "default", enabled: true, firing: false }),
    ]);
    const alert = await createAlert(input);
    expect(alert.id).toBe(1);
    expect(calls[0]!.url).toBe("/api/alerts");
    expect(calls[0]!.init.method).toBe("POST");
    expect(new Headers(calls[0]!.init.headers).get("Content-Type")).toBe(
      "application/json",
    );
    expect(JSON.parse(String(calls[0]!.init.body))).toMatchObject({
      threshold: 0.5,
      webhookUrl: "https://hooks.example.test/hook",
    });
  });

  it("posts a bounded evaluate request and decodes cap/skip counts", async () => {
    const calls = stubFetch([
      json({
        runsEvaluated: 12,
        runsSkipped: 3,
        runLimit: 15,
        judgeCalls: 4,
        judgeCallsSkipped: 0,
        judgeCallLimit: 100,
      }),
    ]);
    const report = await evaluateAssertions({ project: "prod", limit: 15 });
    expect(calls[0]!.url).toBe(
      "/api/assertions/evaluate?project=prod&limit=15",
    );
    expect(calls[0]!.init.method).toBe("POST");
    expect(report.runsSkipped).toBe(3);
    expect(report.judgeCallLimit).toBe(100);
  });

  it("lists tokens without exposing a secret field", async () => {
    stubFetch([
      json({
        tokens: [
          {
            id: "tok_1",
            name: "ci",
            scope: "read",
            createdAt: "2026-11-04T09:00:00Z",
          },
        ],
      }),
    ]);
    const tokens = await fetchTokens();
    expect(tokens[0]?.scope).toBe("read");
    expect("token" in (tokens[0] ?? {})).toBe(false);
  });

  it("returns the minted token secret only from createToken", async () => {
    stubFetch([
      json({
        id: "tok_1",
        token: "secret",
        name: "ci",
        scope: "admin",
        createdAt: "2026-11-04T09:00:00Z",
      }),
    ]);
    const created = await createToken({ name: "ci", scope: "admin" });
    expect(created.token).toBe("secret");
    expect(created.scope).toBe("admin");
  });

  it("reads the Prometheus metrics text endpoint", async () => {
    stubFetch([
      new Response("otterscope_runs_total 1\n", {
        headers: { "Content-Type": "text/plain; version=0.0.4" },
      }),
    ]);
    expect(await fetchMetricsText()).toContain("otterscope_runs_total");
  });
});
