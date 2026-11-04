import { createElement, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "bun:test";
import type { RunDetail } from "@/api";
import { RunView } from "@/components/run-view";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let host: HTMLDivElement;
let root: Root;

const render = async (detail: RunDetail) => {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => {
    root.render(createElement(RunView, { detail }));
  });
};

const click = async (element: Element) => {
  await act(async () => {
    element.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
};

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

const detail: RunDetail = {
  run: {
    id: "abc123",
    project: "default",
    service: "checkout",
    agentName: "planner",
    status: "ok",
    start: "2026-11-01T10:00:00.000Z",
    durationMs: 2000,
    inputTokens: 1200,
    outputTokens: 500,
    llmCalls: 1,
    toolCalls: 1,
    models: "claude-sonnet-5",
    costUsd: 0.012,
    error: "",
  },
  assertionResults: [
    {
      assertionId: 1,
      name: "polite",
      type: "llm_judge",
      pass: true,
      detail: "sounds fine",
    },
    {
      assertionId: 2,
      name: "json",
      type: "tool_called",
      pass: false,
      detail: "missing tool",
    },
  ],
  steps: [
    {
      id: "s1",
      parentId: "",
      kind: "llm",
      name: "plan",
      status: "ok",
      start: "2026-11-01T10:00:00.500Z",
      offsetMs: 500,
      durationMs: 500,
      llm: {
        provider: "anthropic",
        requestModel: "claude-sonnet-5",
        responseModel: "claude-sonnet-5",
        inputTokens: 1000,
        outputTokens: 500,
        cacheReadTokens: 200,
        reasoningTokens: 100,
        costUsd: 0.01,
        inputMessages: [{ role: "user", content: "hello" }],
        outputMessages: [{ role: "assistant", content: "world" }],
      },
    },
    {
      id: "s2",
      parentId: "s1",
      kind: "tool",
      name: "search",
      status: "error",
      start: "2026-11-01T10:00:01.000Z",
      offsetMs: 1000,
      durationMs: 400,
      tool: {
        name: "search",
        callId: "call-1",
        arguments: '{"q":"otters"}',
        result: "not found",
      },
    },
  ],
};

describe("RunView", () => {
  it("renders assertion chips with their pass state", async () => {
    await render(detail);
    expect(host.textContent).toContain("✓ polite");
    expect(host.textContent).toContain("✗ json");
  });

  it("lays the timeline bars out proportionally", async () => {
    await render(detail);
    const bars = host.querySelectorAll(".bar");
    expect(bars).toHaveLength(2);
    expect(bars[0]!.getAttribute("style")).toContain("left: 25%");
    expect(bars[0]!.getAttribute("style")).toContain("width: 25%");
  });

  it("opens the LLM inspector with tokens, cache and reasoning", async () => {
    await render(detail);
    await click(host.querySelectorAll(".step")[0]!);
    expect(host.querySelector(".inspector")).not.toBeNull();
    expect(host.textContent).toContain("anthropic · claude-sonnet-5");
    expect(host.textContent).toContain("1.0k in (200 cached)");
    expect(host.textContent).toContain("500 out (100 reasoning)");
    expect(host.textContent).toContain("hello");
    expect(host.textContent).toContain("world");
  });

  it("shows tool arguments and results and toggles the panel shut", async () => {
    await render(detail);
    const toolStep = host.querySelectorAll(".step")[1]!;
    await click(toolStep);
    expect(host.textContent).toContain('{"q":"otters"}');
    expect(host.textContent).toContain("not found");
    await click(toolStep);
    expect(host.querySelector(".inspector")).toBeNull();
  });
});
