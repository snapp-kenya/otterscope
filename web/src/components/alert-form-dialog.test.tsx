import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "bun:test";
import type { AlertInput } from "@/api";
import { AlertFormDialog } from "@/components/alert-form-dialog";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let host: HTMLDivElement;
let root: Root;

const render = async (onSubmit: (body: AlertInput) => void) => {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => {
    root.render(
      createElement(AlertFormDialog, {
        onOpenChange: () => {},
        initial: null,
        pending: false,
        error: null,
        onSubmit,
      }),
    );
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

const clickAdd = async () => {
  const button = [...document.querySelectorAll("button")].find((element) =>
    (element.textContent ?? "").includes("add alert"),
  );
  if (!button) throw new Error("no add alert button");
  await act(async () => {
    button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
};

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

describe("AlertFormDialog", () => {
  it("refuses a percentage threshold instead of sending NaN", async () => {
    const submitted: AlertInput[] = [];
    await render((body) => submitted.push(body));

    await setInput("alert name", "errors");
    await setInput("webhook URL", "https://example.test/hook");
    await setInput("threshold", "10%");
    await clickAdd();

    expect(document.body.textContent).toContain("Percentages are not accepted");
    expect(submitted).toHaveLength(0);
  });

  it("still sends a finite threshold through", async () => {
    const submitted: AlertInput[] = [];
    await render((body) => submitted.push(body));

    await setInput("alert name", "errors");
    await setInput("webhook URL", "https://example.test/hook");
    await setInput("threshold", "0.25");
    await clickAdd();

    expect(submitted).toHaveLength(1);
    expect(submitted[0]!.threshold).toBe(0.25);
  });
});
