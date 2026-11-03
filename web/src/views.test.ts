// These views are components, and the defects were in how they render a
// settled failure, a truncated page and a filtered-to-nothing result — none of
// which a pure helper can show. So they are driven through React in happy-dom
// with a controllable fetch, the same way compare.test.ts does.
import { act, createElement, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import RunsList from "./RunsList";
import RunDetail from "./RunDetail";
import Audit from "./Audit";
import Alerts from "./Alerts";
import SavedViews from "./SavedViews";
import { withFilterDefaults } from "./filterState";
import type { Run } from "./api";

const currentFilters = () => withFilterDefaults({});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const run = (id: string): Run => ({
  id,
  service: "svc",
  agentName: "agent",
  status: "ok",
  start: new Date("2026-11-01T10:00:00Z").toISOString(),
  durationMs: 1000,
  inputTokens: 10,
  outputTokens: 20,
  llmCalls: 1,
  toolCalls: 2,
  models: "m",
  error: "",
});

const runs = (n: number) => Array.from({ length: n }, (_, i) => run(`run-${i}`));

// A server that answers anything but the runs list, so a test can fail one
// endpoint without taking the page down with it.
const serving = (routes: (url: string, init?: RequestInit) => Response | null) =>
  vi.fn(async (url: string, init?: RequestInit) => routes(url, init) ?? json({}));

let host: HTMLDivElement;
let root: Root;

async function render(el: ReactElement) {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => {
    root.render(el);
  });
  return host;
}

async function click(label: string) {
  const btn = [...host.querySelectorAll("button")].find((b) =>
    (b.textContent ?? "").includes(label),
  );
  if (!btn) throw new Error(`no button matching ${label}`);
  await act(async () => {
    btn.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  });
}

async function type(placeholder: string, value: string) {
  const input = [...host.querySelectorAll("input")].find(
    (i) => i.placeholder === placeholder,
  );
  if (!input) throw new Error(`no input with placeholder ${placeholder}`);
  const set = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    "value",
  )!.set!;
  await act(async () => {
    set.call(input, value);
    input.dispatchEvent(new window.Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  window.history.replaceState(null, "", "/");
  // useLiveTick opens an SSE connection no test needs; a stub keeps the
  // safety interval from holding the process open.
  class NoEvents {
    addEventListener() {}
    close() {}
    onerror: (() => void) | null = null;
  }
  vi.stubGlobal("EventSource", NoEvents);
});

afterEach(async () => {
  await act(async () => {
    root?.unmount();
  });
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("RunsList", () => {
  // Every view used to `.catch(() => {})` and treat null as "loading", so a
  // 500 left the page saying "loading…" for as long as it stayed open.
  it("settles a failed fetch as an error with a retry, never as loading", async () => {
    vi.stubGlobal(
      "fetch",
      serving((url) =>
        url.includes("/api/views")
          ? json({ views: [] })
          : json({ error: "query failed" }, 500),
      ),
    );
    await render(createElement(RunsList, { navigate: () => {} }));

    expect(host.textContent).toContain("query failed");
    expect(host.textContent).toContain("retry");
    expect(host.textContent).not.toContain("loading…");
  });

  // The list fetched one fixed page of 100 and said nothing about the rest, so
  // run 101 was unreachable.
  it("pages with offset and admits when there is more", async () => {
    const fetchMock = serving((url) => {
      if (url.includes("/api/views")) return json({ views: [] });
      const offset = new URLSearchParams(url.split("?")[1]).get("offset");
      return json({ runs: offset ? runs(1) : runs(101) });
    });
    vi.stubGlobal("fetch", fetchMock);
    await render(createElement(RunsList, { navigate: () => {} }));

    expect(host.textContent).toContain("showing 1–100");
    expect(host.textContent).toContain("more match");
    // The 101st row is lookahead evidence, not a row on the page.
    expect(host.querySelectorAll("tbody tr")).toHaveLength(100);

    await click("older");
    const urls = fetchMock.mock.calls.map((c) => String(c[0]));
    expect(urls.some((u) => u.includes("offset=100"))).toBe(true);
    expect(host.textContent).toContain("showing 101–101");
    expect(host.textContent).toContain("end of the list");
  });

  // `hasFilters` omitted the text query, so a search with no hits rendered
  // "No runs yet." and the onboarding copy.
  it("does not call an empty search 'No runs yet'", async () => {
    window.history.replaceState(null, "", "/?q=nothingmatchesthis");
    vi.stubGlobal(
      "fetch",
      serving((url) =>
        url.includes("/api/views") ? json({ views: [] }) : json({ runs: [] }),
      ),
    );
    await render(createElement(RunsList, { navigate: () => {} }));

    expect(host.textContent).toContain("No runs match these filters.");
    expect(host.textContent).not.toContain("No runs yet.");
    expect(host.textContent).not.toContain("OTEL_EXPORTER_OTLP_ENDPOINT");
  });

  it("keeps the onboarding copy when nothing is filtered", async () => {
    vi.stubGlobal(
      "fetch",
      serving((url) =>
        url.includes("/api/views") ? json({ views: [] }) : json({ runs: [] }),
      ),
    );
    await render(createElement(RunsList, { navigate: () => {} }));

    expect(host.textContent).toContain("No runs yet.");
    expect(host.textContent).toContain("OTEL_EXPORTER_OTLP_ENDPOINT");
  });
});

describe("RunDetail", () => {
  it("settles a failed fetch as an error with a retry", async () => {
    vi.stubGlobal("fetch", serving(() => json({ error: "query failed" }, 500)));
    await render(createElement(RunDetail, { id: "run-1", navigate: () => {} }));

    expect(host.textContent).toContain("query failed");
    expect(host.textContent).toContain("retry");
    expect(host.textContent).not.toContain("loading…");
  });

  it("still distinguishes a real 404", async () => {
    vi.stubGlobal("fetch", serving(() => json({ error: "run not found" }, 404)));
    await render(createElement(RunDetail, { id: "gone", navigate: () => {} }));

    expect(host.textContent).toContain("Run not found.");
  });
});

describe("Audit", () => {
  it("settles a failed fetch as an error with a retry", async () => {
    vi.stubGlobal("fetch", serving(() => json({ error: "query failed" }, 500)));
    await render(createElement(Audit));

    expect(host.textContent).toContain("query failed");
    expect(host.textContent).toContain("retry");
    expect(host.textContent).not.toContain("loading…");
  });
});

describe("Alerts", () => {
  // Typing 10% became Number("10%") = NaN, JSON wrote null, the server stored
  // 0, and the alert fired on every evaluation.
  it("refuses a percentage threshold instead of sending NaN", async () => {
    const fetchMock = serving((url) =>
      url.includes("/api/alerts") ? json({ alerts: [] }) : json({}),
    );
    vi.stubGlobal("fetch", fetchMock);
    await render(createElement(Alerts));

    await type("name", "errors");
    await type("webhook URL (Slack/Discord/…)", "https://example.test/hook");
    await type("threshold", "10%");
    await click("add alert");

    expect(host.textContent).toContain("Percentages are not accepted");
    const posted = fetchMock.mock.calls.some(
      (c) => (c[1] as RequestInit | undefined)?.method === "POST",
    );
    expect(posted).toBe(false);
  });

  it("still sends a finite threshold through", async () => {
    const fetchMock = serving((url) =>
      url.includes("/api/alerts") ? json({ alerts: [] }) : json({}),
    );
    vi.stubGlobal("fetch", fetchMock);
    await render(createElement(Alerts));

    await type("name", "errors");
    await type("webhook URL (Slack/Discord/…)", "https://example.test/hook");
    await type("threshold", "0.25");
    await click("add alert");

    const post = fetchMock.mock.calls.find(
      (c) => (c[1] as RequestInit | undefined)?.method === "POST",
    );
    expect(post, "a valid threshold must still reach the API").toBeTruthy();
    expect(String((post![1] as RequestInit).body)).toContain('"threshold":0.25');
  });

  it("settles a failed list fetch as an error with a retry", async () => {
    vi.stubGlobal("fetch", serving(() => json({ error: "query failed" }, 500)));
    await render(createElement(Alerts));

    expect(host.textContent).toContain("Couldn't load alerts");
    expect(host.textContent).toContain("retry");
  });
});

describe("SavedViews", () => {
  const views = (...names: string[]) => ({
    views: names.map((name, i) => ({ id: i + 1, name, params: {} })),
  });
  const renderViews = () =>
    render(
      createElement(SavedViews, {
        current: currentFilters(),
        onApply: () => {},
      }),
    );

  // This view kept `.catch(() => {})` and left `views` null, so the next
  // render called `.map` on it: a 500 on /api/views threw during render and
  // took the whole run list down with it.
  it("reports a failed load instead of throwing the page away", async () => {
    vi.stubGlobal(
      "fetch",
      serving(() => json({ error: "store unavailable" }, 500)),
    );
    await renderViews();

    expect(host.textContent).toContain("Couldn't read saved views");
    expect(host.textContent).toContain("store unavailable");
    expect(host.textContent).toContain("retry");
    // No presets were read, so none may be invented or mapped over.
    expect(host.querySelectorAll(".view-chip")).toHaveLength(0);
  });

  it("refetches on retry and shows the views it gets back", async () => {
    let calls = 0;
    vi.stubGlobal(
      "fetch",
      serving(() => {
        calls += 1;
        return calls === 1
          ? json({ error: "store unavailable" }, 500)
          : json(views("prod errors"));
      }),
    );
    await renderViews();
    expect(host.textContent).toContain("Couldn't read saved views");

    await click("retry");

    expect(host.textContent).toContain("prod errors");
    expect(host.textContent).not.toContain("Couldn't read saved views");
  });

  // A body without a `views` array used to hand undefined straight to state.
  it("refuses a body that is not a view list", async () => {
    vi.stubGlobal("fetch", serving(() => json({})));
    await renderViews();

    expect(host.textContent).toContain("did not return a list of views");
    expect(host.querySelectorAll(".view-chip")).toHaveLength(0);
  });

  // Deleting used to ignore the response: a refused delete left the chip on
  // screen and said nothing at all.
  it("reports why a delete failed", async () => {
    vi.stubGlobal(
      "fetch",
      serving((_url, init) =>
        init?.method === "DELETE"
          ? json({ error: "view is locked" }, 500)
          : json(views("prod errors")),
      ),
    );
    await renderViews();

    await click("×");

    expect(host.textContent).toContain("Couldn't delete view");
    expect(host.textContent).toContain("view is locked");
    // It was not deleted, so it must still be there to try again.
    expect(host.textContent).toContain("prod errors");
  });

  it("reports why a save failed", async () => {
    // happy-dom has no window.prompt at all, so stub it rather than spy.
    vi.stubGlobal("prompt", vi.fn(() => "prod errors"));
    vi.stubGlobal(
      "fetch",
      serving((_url, init) =>
        init?.method === "POST"
          ? json({ error: "duplicate name" }, 409)
          : json(views()),
      ),
    );
    await renderViews();

    await click("save view");

    expect(host.textContent).toContain("Couldn't save view");
    expect(host.textContent).toContain("duplicate name");
  });
});
