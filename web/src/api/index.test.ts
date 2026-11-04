import { afterEach, describe, expect, it } from "bun:test";
import {
  ApiError,
  createAlert,
  createAssertion,
  createToken,
  createView,
  deleteAlert,
  deleteAssertion,
  deleteToken,
  deleteView,
  evaluateAssertions,
  fetchAlerts,
  fetchMetricsText,
  fetchRuns,
  fetchSharedRun,
  fetchTokens,
  fetchViews,
  mintShare,
  revokeShare,
  type Alert,
  type AlertInput,
  type AssertionInput,
} from "@/api";

type FetchCall = { url: string; init: RequestInit };

const originalFetch = globalThis.fetch;
const originalPrompt = window.prompt;

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

const alertInput: AlertInput = {
  name: "errors",
  type: "error_rate",
  threshold: 0.5,
  config: "",
  windowSecs: 3600,
  settleSecs: 300,
  webhookUrl: "https://hooks.example.test/hook",
};

const alertRecord: Alert = {
  id: 5,
  project: "default",
  name: "errors",
  type: "error_rate",
  threshold: 0.5,
  windowSecs: 3600,
  config: "",
  webhookUrl: "https://hooks.example.test/hook",
  enabled: true,
  firing: false,
  settleSecs: 300,
};

const savedView = {
  id: 7,
  name: "prod errors",
  params: { status: "error" },
};

const assertionInput: AssertionInput = {
  name: "has answer",
  type: "contains",
  config: "answer",
};

const assertionRecord = {
  id: 2,
  project: "default",
  name: "has answer",
  type: "contains",
  config: "answer",
  enabled: true,
};

const evaluateReport = {
  runsEvaluated: 12,
  runsSkipped: 3,
  runLimit: 15,
  judgeCalls: 4,
  judgeCallsSkipped: 0,
  judgeCallLimit: 100,
};

const createdToken = {
  id: "tok_1",
  token: "secret",
  name: "ci",
  scope: "admin",
  createdAt: "2026-11-04T09:00:00Z",
};

type AdminMutation = {
  label: string;
  call: () => Promise<unknown>;
  success: () => Response;
  path: string;
  method: string;
};

const adminMutations: AdminMutation[] = [
  {
    label: "saving a view",
    call: () => createView("prod errors", { status: "error" }),
    success: () => json(savedView, 201),
    path: "/api/views",
    method: "POST",
  },
  {
    label: "deleting a view",
    call: () => deleteView(savedView.id),
    success: () => new Response(null, { status: 204 }),
    path: "/api/views/7",
    method: "DELETE",
  },
  {
    label: "creating an alert",
    call: () => createAlert(alertInput),
    success: () => json(alertRecord, 201),
    path: "/api/alerts",
    method: "POST",
  },
  {
    label: "deleting an alert",
    call: () => deleteAlert(alertRecord.id),
    success: () => new Response(null, { status: 204 }),
    path: "/api/alerts/5",
    method: "DELETE",
  },
  {
    label: "creating an assertion",
    call: () => createAssertion(assertionInput),
    success: () => json(assertionRecord, 201),
    path: "/api/assertions",
    method: "POST",
  },
  {
    label: "deleting an assertion",
    call: () => deleteAssertion(assertionRecord.id),
    success: () => new Response(null, { status: 204 }),
    path: "/api/assertions/2",
    method: "DELETE",
  },
  {
    label: "evaluating assertions",
    call: () => evaluateAssertions({ project: "prod", limit: 15 }),
    success: () => json(evaluateReport),
    path: "/api/assertions/evaluate?project=prod&limit=15",
    method: "POST",
  },
  {
    label: "creating a token",
    call: () => createToken({ name: "ci", scope: "admin" }),
    success: () => json(createdToken, 201),
    path: "/api/tokens",
    method: "POST",
  },
  {
    label: "deleting a token",
    call: () => deleteToken(createdToken.id),
    success: () => new Response(null, { status: 204 }),
    path: "/api/tokens/tok_1",
    method: "DELETE",
  },
];

const stubPrompt = (answer: string | null): (() => number) => {
  let prompts = 0;
  window.prompt = (() => {
    prompts += 1;
    return answer;
  }) as typeof window.prompt;
  return () => prompts;
};

const adminRequired = () => json({ error: "admin token required" }, 403);

const authHeader = (call: FetchCall) =>
  new Headers(call.init.headers).get("Authorization");

afterEach(() => {
  globalThis.fetch = originalFetch;
  window.prompt = originalPrompt;
  localStorage.clear();
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

describe("share admin recovery", () => {
  it("prompts for an admin token on 403 and retries minting once", async () => {
    localStorage.setItem("otterscope_token", "read-token");
    const calls = stubFetch([
      adminRequired(),
      json({ token: "share-token", url: "/otterscope/s/share-token" }, 201),
    ]);
    const prompts = stubPrompt("  admin-token  ");

    const share = await mintShare("run-1");

    expect(share.token).toBe("share-token");
    expect(calls).toHaveLength(2);
    expect(authHeader(calls[0]!)).toBe("Bearer read-token");
    expect(authHeader(calls[1]!)).toBe("Bearer admin-token");
    expect(prompts()).toBe(1);
    expect(localStorage.getItem("otterscope_token")).toBe("admin-token");
  });

  it("surfaces the 403 after one failed mint retry without prompting again", async () => {
    const calls = stubFetch([adminRequired(), adminRequired()]);
    const prompts = stubPrompt("admin-token");

    try {
      await mintShare("run-1");
      throw new Error("expected the mint to fail");
    } catch (error) {
      expect(error).toBeInstanceOf(ApiError);
      if (error instanceof ApiError) {
        expect(error.status).toBe(403);
        expect(error.message).toContain("admin token required");
      }
    }

    expect(calls).toHaveLength(2);
    expect(prompts()).toBe(1);
  });

  it("prompts and retries revoking a share link", async () => {
    localStorage.setItem("otterscope_token", "read-token");
    const calls = stubFetch([
      adminRequired(),
      new Response(null, { status: 204 }),
    ]);
    const prompts = stubPrompt("admin-token");

    await revokeShare("share-token");

    expect(calls).toHaveLength(2);
    expect(calls[1]!.init.method).toBe("DELETE");
    expect(authHeader(calls[1]!)).toBe("Bearer admin-token");
    expect(prompts()).toBe(1);
  });

  it("surfaces the 403 after one failed revoke retry", async () => {
    const calls = stubFetch([adminRequired(), adminRequired()]);
    const prompts = stubPrompt("admin-token");

    try {
      await revokeShare("share-token");
      throw new Error("expected the revoke to fail");
    } catch (error) {
      expect(error).toBeInstanceOf(ApiError);
      if (error instanceof ApiError) expect(error.status).toBe(403);
    }

    expect(calls).toHaveLength(2);
    expect(prompts()).toBe(1);
  });

  it("does not retry when the admin prompt is dismissed", async () => {
    const calls = stubFetch([adminRequired()]);
    const prompts = stubPrompt(null);

    try {
      await mintShare("run-1");
      throw new Error("expected the mint to fail");
    } catch (error) {
      expect(error).toBeInstanceOf(ApiError);
    }

    expect(calls).toHaveLength(1);
    expect(prompts()).toBe(1);
  });

  it("does not prompt for a 403 that is not about admin scope", async () => {
    const calls = stubFetch([json({ error: "origin not allowed" }, 403)]);
    const prompts = stubPrompt("admin-token");

    try {
      await mintShare("run-1");
      throw new Error("expected the mint to fail");
    } catch (error) {
      expect(error).toBeInstanceOf(ApiError);
    }

    expect(calls).toHaveLength(1);
    expect(prompts()).toBe(0);
  });
});

describe("admin recovery for every admin-scoped mutation", () => {
  for (const mutation of adminMutations) {
    it(`prompts once for an admin token and retries ${mutation.label}`, async () => {
      localStorage.setItem("otterscope_token", "read-token");
      const calls = stubFetch([adminRequired(), mutation.success()]);
      const prompts = stubPrompt("  admin-token  ");

      await mutation.call();

      expect(calls).toHaveLength(2);
      expect(calls[0]!.url).toBe(mutation.path);
      expect(calls[0]!.init.method).toBe(mutation.method);
      expect(authHeader(calls[0]!)).toBe("Bearer read-token");
      expect(calls[1]!.url).toBe(mutation.path);
      expect(calls[1]!.init.method).toBe(mutation.method);
      expect(authHeader(calls[1]!)).toBe("Bearer admin-token");
      expect(prompts()).toBe(1);
      expect(localStorage.getItem("otterscope_token")).toBe("admin-token");
    });

    it(`surfaces the second 403 for ${mutation.label}`, async () => {
      const calls = stubFetch([adminRequired(), adminRequired()]);
      const prompts = stubPrompt("admin-token");

      try {
        await mutation.call();
        throw new Error(`expected ${mutation.label} to fail`);
      } catch (error) {
        expect(error).toBeInstanceOf(ApiError);
        if (error instanceof ApiError) {
          expect(error.status).toBe(403);
          expect(error.message).toContain("admin token required");
        }
      }

      expect(calls).toHaveLength(2);
      expect(prompts()).toBe(1);
    });
  }

  it("does not prompt for a save-view 403 that is not about admin scope", async () => {
    const calls = stubFetch([json({ error: "origin not allowed" }, 403)]);
    const prompts = stubPrompt("admin-token");

    try {
      await createView("prod errors", {});
      throw new Error("expected the save to fail");
    } catch (error) {
      expect(error).toBeInstanceOf(ApiError);
      if (error instanceof ApiError) {
        expect(error.message).toContain("origin not allowed");
      }
    }

    expect(calls).toHaveLength(1);
    expect(prompts()).toBe(0);
  });

  it("does not retry an alert create when the admin prompt is dismissed", async () => {
    const calls = stubFetch([adminRequired()]);
    const prompts = stubPrompt(null);

    try {
      await createAlert(alertInput);
      throw new Error("expected the create to fail");
    } catch (error) {
      expect(error).toBeInstanceOf(ApiError);
    }

    expect(calls).toHaveLength(1);
    expect(prompts()).toBe(1);
  });
});

describe("a read-token user saves a view and creates an alert", () => {
  it("recovers both mutations with a single admin prompt", async () => {
    localStorage.setItem("otterscope_token", "read-token");
    const storedViews: { id: number; name: string; params: unknown }[] = [];
    const storedAlerts: Alert[] = [];
    let prompts = 0;
    window.prompt = (() => {
      prompts += 1;
      return "admin-token";
    }) as typeof window.prompt;

    globalThis.fetch = (async (
      input: RequestInfo | URL,
      init?: RequestInit,
    ): Promise<Response> => {
      const url = String(input);
      const method = init?.method ?? "GET";
      const auth = new Headers(init?.headers).get("Authorization");
      if (auth !== "Bearer admin-token") return adminRequired();
      if (url === "/api/views" && method === "POST") {
        const body = JSON.parse(String(init?.body)) as {
          name: string;
          params: unknown;
        };
        const view = {
          id: storedViews.length + 1,
          name: body.name,
          params: body.params,
        };
        storedViews.push(view);
        return json(view, 201);
      }
      if (url === "/api/views") return json({ views: storedViews });
      if (url === "/api/alerts" && method === "POST") {
        const body = JSON.parse(String(init?.body)) as AlertInput;
        const alert: Alert = {
          id: storedAlerts.length + 1,
          project: body.project ?? "default",
          name: body.name,
          type: body.type,
          threshold: body.threshold,
          windowSecs: body.windowSecs ?? 3600,
          config: body.config ?? "",
          webhookUrl: body.webhookUrl,
          enabled: true,
          firing: false,
          settleSecs: body.settleSecs ?? 300,
        };
        storedAlerts.push(alert);
        return json(alert, 201);
      }
      if (url === "/api/alerts") return json({ alerts: storedAlerts });
      return json({ error: "unexpected request" }, 500);
    }) as typeof fetch;

    const view = await createView("prod errors", { status: "error" });
    const alert = await createAlert(alertInput);
    const views = await fetchViews();
    const alerts = await fetchAlerts();

    expect(prompts).toBe(1);
    expect(localStorage.getItem("otterscope_token")).toBe("admin-token");
    expect(view.name).toBe("prod errors");
    expect(alert.name).toBe("errors");
    expect(views.map((saved) => saved.name)).toEqual(["prod errors"]);
    expect(alerts.map((listed) => listed.id)).toEqual([alert.id]);
  });
});
