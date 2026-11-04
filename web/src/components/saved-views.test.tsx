import { createElement, act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "bun:test";
import { emptyFilters } from "@/lib/filters";
import { SavedViews } from "@/components/saved-views";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const originalFetch = globalThis.fetch;
let host: HTMLDivElement;
let root: Root;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const stubFetch = (handler: (init?: RequestInit) => Response) => {
  const calls: { url: string; init: RequestInit }[] = [];
  globalThis.fetch = (async (
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> => {
    calls.push({ url: String(input), init: init ?? {} });
    return handler(init);
  }) as typeof fetch;
  return calls;
};

const render = async (element: ReactElement) => {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => {
    root.render(
      createElement(
        QueryClientProvider,
        { client: queryClient },
        element,
      ),
    );
  });
};

const flush = async () => {
  for (let round = 0; round < 4; round += 1) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
  }
};

const button = (label: string) => {
  const found = [...host.querySelectorAll("button")].find((element) =>
    (element.textContent ?? "").includes(label),
  );
  if (!found) throw new Error(`no button matching ${label}`);
  return found;
};

const click = async (element: Element) => {
  await act(async () => {
    element.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
};

const views = (...names: string[]) => ({
  views: names.map((name, index) => ({ id: index + 1, name, params: {} })),
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  globalThis.fetch = originalFetch;
});

const savedViews = () =>
  createElement(SavedViews, {
    current: { ...emptyFilters },
    onApply: () => {},
  });

describe("SavedViews", () => {
  it("settles a failed load as an error with a retry", async () => {
    stubFetch(() => json({ error: "store unavailable" }, 500));
    await render(savedViews());
    await flush();

    expect(host.textContent).toContain("Couldn't read saved views");
    expect(host.textContent).toContain("store unavailable");
    expect(host.textContent).toContain("retry");
    expect(host.querySelectorAll(".view-chip")).toHaveLength(0);
  });

  it("refetches on retry and renders the views it gets back", async () => {
    let calls = 0;
    stubFetch(() => {
      calls += 1;
      return calls === 1
        ? json({ error: "store unavailable" }, 500)
        : json(views("prod errors"));
    });
    await render(savedViews());
    await flush();
    expect(host.textContent).toContain("Couldn't read saved views");

    await click(button("retry"));
    await flush();

    expect(host.textContent).toContain("prod errors");
    expect(host.textContent).not.toContain("Couldn't read saved views");
  });

  it("treats a body without a view list as empty rather than throwing", async () => {
    stubFetch(() => json({}));
    await render(savedViews());
    await flush();

    expect(host.querySelectorAll(".view-chip")).toHaveLength(0);
    expect(host.textContent).not.toContain("Couldn't read saved views");
  });

  it("applies a view through the filter defaults", async () => {
    const applied: unknown[] = [];
    stubFetch(() =>
      json({
        views: [{ id: 1, name: "alpha", params: { model: "claude" } }],
      }),
    );
    await render(
      createElement(SavedViews, {
        current: { ...emptyFilters },
        onApply: (filters) => applied.push(filters),
      }),
    );
    await flush();

    await click(button("alpha"));
    expect(applied).toEqual([
      { ...emptyFilters, model: "claude" },
    ]);
  });

  it("reports why a delete failed and keeps the chip", async () => {
    stubFetch((init) =>
      init?.method === "DELETE"
        ? json({ error: "view is locked" }, 500)
        : json(views("prod errors")),
    );
    await render(savedViews());
    await flush();

    const deleteButton = host.querySelector(
      '[aria-label="delete view prod errors"]',
    );
    if (!deleteButton) throw new Error("no delete button for prod errors");
    await click(deleteButton);
    await flush();

    expect(host.textContent).toContain("Couldn't delete view");
    expect(host.textContent).toContain("view is locked");
    expect(host.textContent).toContain("prod errors");
  });
});
