import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Compare from "./Compare";

// The component, not a pure helper, is where the race lived: a request per
// keystroke, none cancelled, and whichever response finished last won. These
// drive it through React with a controllable fetch so the ordering is exact.

type Pending = {
  url: string;
  signal: AbortSignal | null;
  resolve: (body: unknown) => void;
};

let pending: Pending[] = [];
let container: HTMLDivElement;
let root: Root;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const stats = (runs: number) => ({
  runs,
  errors: 0,
  avgDurationMs: 0,
  p50DurationMs: 0,
  p95DurationMs: 0,
  inputTokens: 0,
  outputTokens: 0,
  assertionRates: [],
});

const flush = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

const advance = (ms: number) =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
    await flush();
  });

beforeEach(() => {
  pending = [];
  window.history.replaceState(null, "", "/compare?a_model=one");
  vi.useFakeTimers();
  vi.stubGlobal(
    "fetch",
    vi.fn(
      (input: RequestInfo | URL, init?: RequestInit) =>
        new Promise<Response>((resolve) => {
          pending.push({
            url: String(input),
            signal: init?.signal ?? null,
            resolve: (body) =>
              resolve({
                ok: true,
                status: 200,
                json: async () => body,
              } as Response),
          });
        }),
    ),
  );
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const render = async () => {
  await act(async () => {
    root.render(createElement(Compare));
  });
};

const modelInput = () =>
  container.querySelectorAll<HTMLInputElement>('input[placeholder="model…"]')[0];

// React tracks the previous value on the DOM node, so setting `.value` alone
// is ignored; use the native setter and dispatch the event React listens for.
const type = async (el: HTMLInputElement, value: string) => {
  const setter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

const cell = (row: number, col: number) =>
  container.querySelectorAll("tbody tr")[row].querySelectorAll("td")[col]
    .textContent;

describe("Compare request handling", () => {
  it("waits for a pause in typing before asking the server", async () => {
    await render();
    expect(pending).toHaveLength(0);

    await advance(249);
    expect(pending).toHaveLength(0);

    await advance(1);
    expect(pending).toHaveLength(2);
    expect(pending[0].url).toContain("model=one");
  });

  it("collapses a burst of keystrokes into one request per side", async () => {
    await render();
    const input = modelInput();
    for (const v of ["c", "cl", "cla", "clau", "claude"]) await type(input, v);

    await advance(250);
    expect(pending).toHaveLength(2);
    expect(pending[0].url).toContain("model=claude");
    expect(pending[1].url).not.toContain("model=");
  });

  it("cancels the in-flight request when the inputs change", async () => {
    await render();
    const input = modelInput();
    await type(input, "slow");
    await advance(250);
    expect(pending).toHaveLength(2);

    await type(input, "fast");
    expect(pending[0].signal?.aborted).toBe(true);
    expect(pending[1].signal?.aborted).toBe(true);
  });

  // The bug: the slow response arrived after the fast one and painted numbers
  // for a query the input no longer held. Abort alone does not stop a response
  // already on the wire, so the sequence check has to drop it.
  it("ignores an older response that lands last", async () => {
    await render();
    const input = modelInput();
    await type(input, "slow");
    await advance(250);
    const [slowA, slowB] = pending;

    await type(input, "fast");
    await advance(250);
    expect(pending).toHaveLength(4);
    const [, , fastA, fastB] = pending;

    await act(async () => {
      fastA.resolve(stats(4));
      fastB.resolve(stats(4));
      await flush();
    });
    expect(cell(0, 1)).toBe("4");
    expect(cell(0, 2)).toBe("4");

    // Resolve the superseded pair as if it ignored the abort.
    await act(async () => {
      slowA.resolve(stats(99));
      slowB.resolve(stats(99));
      await flush();
    });
    expect(cell(0, 1)).toBe("4");
    expect(cell(0, 2)).toBe("4");
  });
});
