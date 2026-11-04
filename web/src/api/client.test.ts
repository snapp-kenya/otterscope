import { afterEach, describe, expect, it } from "bun:test";
import {
  apiFetch,
  ApiError,
  csvTruncationNotice,
  errorMessage,
  failureMessage,
  readToken,
  transportMessage,
} from "@/api";

type FetchCall = { url: string; init: RequestInit };

const originalFetch = globalThis.fetch;
const originalPrompt = window.prompt;

const captureFetch = (responses: Response[]): FetchCall[] => {
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

afterEach(() => {
  globalThis.fetch = originalFetch;
  window.prompt = originalPrompt;
  localStorage.clear();
});

describe("csvTruncationNotice", () => {
  const response = (headers: Record<string, string>) =>
    new Response("", { headers });

  it("is silent for a complete export", () => {
    expect(csvTruncationNotice(response({}))).toBeNull();
  });

  it("reports the cap when the export was truncated", () => {
    const notice = csvTruncationNotice(
      response({
        "X-Otterscope-Truncated": "true",
        "X-Otterscope-Row-Limit": "10000",
      }),
    );
    expect(notice).toContain("10000");
    expect(notice).toMatch(/narrow the filters/i);
  });

  it("still warns when the limit header is missing", () => {
    const notice = csvTruncationNotice(
      response({ "X-Otterscope-Truncated": "true" }),
    );
    expect(notice).not.toBeNull();
    expect(notice).not.toContain("null");
  });
});

describe("failureMessage", () => {
  it("shows the server's own error next to the status", async () => {
    const res = new Response(JSON.stringify({ error: "query failed" }), {
      status: 500,
    });
    const message = await failureMessage(res);
    expect(message).toContain("500");
    expect(message).toContain("query failed");
  });

  it("falls back to the status alone for a non-JSON body", async () => {
    const res = new Response("<html>bad gateway</html>", { status: 502 });
    expect(await failureMessage(res)).toContain("502");
  });

  it("handles an empty error body without inventing detail", async () => {
    const res = new Response("", { status: 401 });
    const message = await failureMessage(res);
    expect(message).toContain("401");
    expect(message).not.toContain("undefined");
    expect(message).not.toContain("null");
  });
});

describe("transportMessage", () => {
  it("names the browser's reason for a failed fetch", () => {
    const message = transportMessage(new TypeError("Failed to fetch"));
    expect(message).toMatch(/could not reach the server/i);
    expect(message).toContain("Failed to fetch");
  });

  it("still says something for a rejection with no message", () => {
    expect(transportMessage(undefined)).toMatch(/could not reach the server/i);
  });
});

describe("apiFetch", () => {
  it("sends no Authorization header when no token is stored", async () => {
    const calls = captureFetch([new Response("{}")]);
    await apiFetch("/api/runs");
    expect(new Headers(calls[0]!.init.headers).get("Authorization")).toBeNull();
  });

  it("attaches the stored token", async () => {
    localStorage.setItem("otterscope_token", "tok-123");
    const calls = captureFetch([new Response("{}")]);
    await apiFetch("/api/runs");
    expect(new Headers(calls[0]!.init.headers).get("Authorization")).toBe(
      "Bearer tok-123",
    );
    expect(readToken()).toBe("tok-123");
  });

  it("prompts once on 401, stores the token, and retries", async () => {
    const calls = captureFetch([
      new Response("", { status: 401 }),
      new Response("{}", { status: 200 }),
    ]);
    window.prompt = (() => "  entered-token  ") as typeof window.prompt;

    const res = await apiFetch("/api/runs");

    expect(res.status).toBe(200);
    expect(calls).toHaveLength(2);
    expect(readToken()).toBe("entered-token");
    expect(new Headers(calls[1]!.init.headers).get("Authorization")).toBe(
      "Bearer entered-token",
    );
  });

  it("gives up after one retry rather than prompting in a loop", async () => {
    const calls = captureFetch([
      new Response("", { status: 401 }),
      new Response("", { status: 401 }),
    ]);
    let prompts = 0;
    window.prompt = (() => {
      prompts += 1;
      return "nope";
    }) as typeof window.prompt;

    const res = await apiFetch("/api/runs");

    expect(res.status).toBe(401);
    expect(calls).toHaveLength(2);
    expect(prompts).toBe(1);
  });

  it("does not retry when the prompt is dismissed", async () => {
    const calls = captureFetch([new Response("", { status: 401 })]);
    window.prompt = (() => null) as typeof window.prompt;

    const res = await apiFetch("/api/runs");

    expect(res.status).toBe(401);
    expect(calls).toHaveLength(1);
  });
});

describe("errorMessage", () => {
  it("carries the status of an API failure", () => {
    const error = new ApiError("500: query failed", 500);
    expect(error.status).toBe(500);
    expect(errorMessage(error)).toBe("500: query failed");
  });

  it("wraps a transport failure", () => {
    expect(errorMessage(new TypeError("Failed to fetch"))).toMatch(
      /could not reach the server/i,
    );
  });
});
