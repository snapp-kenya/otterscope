import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "bun:test";
import type { Alert, AlertInput } from "@/api";
import { AlertsPage } from "@/routes/alerts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const originalFetch = globalThis.fetch;
const originalPrompt = window.prompt;
let host: HTMLDivElement;
let root: Root;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const render = async () => {
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
        createElement(AlertsPage),
      ),
    );
  });
};

const flush = async () => {
  for (let round = 0; round < 6; round += 1) {
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

const setInput = async (label: string, value: string) => {
  const input = document.querySelector(
    `[aria-label="${label}"]`,
  ) as HTMLInputElement | null;
  if (!input) throw new Error(`no input ${label}`);
  const setter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!;
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  globalThis.fetch = originalFetch;
  window.prompt = originalPrompt;
  localStorage.clear();
});

describe("AlertsPage admin recovery", () => {
  it("creates an alert for a read-token user after one admin prompt", async () => {
    localStorage.setItem("otterscope_token", "read-token");
    const stored: Alert[] = [];
    let prompts = 0;
    window.prompt = (() => {
      prompts += 1;
      return "admin-token";
    }) as typeof window.prompt;

    const calls: { url: string; init: RequestInit }[] = [];
    globalThis.fetch = (async (
      input: RequestInfo | URL,
      init?: RequestInit,
    ): Promise<Response> => {
      calls.push({ url: String(input), init: init ?? {} });
      const method = init?.method ?? "GET";
      if (method === "POST") {
        const auth = new Headers(init?.headers).get("Authorization");
        if (auth !== "Bearer admin-token") {
          return json({ error: "admin token required" }, 403);
        }
        const body = JSON.parse(String(init?.body)) as AlertInput;
        const alert: Alert = {
          id: stored.length + 1,
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
        stored.push(alert);
        return json(alert, 201);
      }
      return json({ alerts: stored });
    }) as typeof fetch;

    await render();
    await flush();

    await click(button("new alert"));
    await flush();
    await setInput("alert name", "errors");
    await setInput("threshold", "0.5");
    await setInput("webhook URL", "https://hooks.example.test/hook");
    await click(button("add alert"));
    await flush();

    expect(prompts).toBe(1);
    expect(localStorage.getItem("otterscope_token")).toBe("admin-token");
    expect(calls.filter((call) => call.init.method === "POST")).toHaveLength(2);
    expect(host.textContent).toContain("errors");
    expect(host.textContent).not.toContain("Couldn't save alert");
  });

  it("keeps the dialog open and names the admin token when the prompt is dismissed", async () => {
    localStorage.setItem("otterscope_token", "read-token");
    window.prompt = (() => null) as typeof window.prompt;

    const calls: { url: string; init: RequestInit }[] = [];
    globalThis.fetch = (async (
      input: RequestInfo | URL,
      init?: RequestInit,
    ): Promise<Response> => {
      calls.push({ url: String(input), init: init ?? {} });
      if ((init?.method ?? "GET") === "POST") {
        return json({ error: "admin token required" }, 403);
      }
      return json({ alerts: [] });
    }) as typeof fetch;

    await render();
    await flush();

    await click(button("new alert"));
    await flush();
    await setInput("alert name", "errors");
    await setInput("webhook URL", "https://hooks.example.test/hook");
    await click(button("add alert"));
    await flush();

    expect(calls.filter((call) => call.init.method === "POST")).toHaveLength(1);
    expect(host.textContent).toContain("admin token required");
    expect(host.textContent).toContain("add alert");
  });
});
