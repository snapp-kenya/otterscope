import { describe, expect, it } from "bun:test";
import {
  alertCondition,
  draftFromAlert,
  emptyAlertDraft,
  shortWebhook,
  validateAlertDraft,
  type AlertDraft,
} from "@/lib/alerts";
import type { Alert } from "@/api";

const draft = (patch: Partial<AlertDraft> = {}): AlertDraft => ({
  ...emptyAlertDraft(),
  name: "errors",
  webhookUrl: "https://hooks.example.test/services/abc",
  ...patch,
});

const alert: Alert = {
  id: 4,
  project: "default",
  name: "errors",
  type: "error_rate",
  threshold: 0.1,
  windowSecs: 3600,
  config: "",
  webhookUrl: "hooks.example.test/…",
  enabled: true,
  firing: false,
  settleSecs: 300,
};

describe("validateAlertDraft", () => {
  it("normalises a valid rule into the request body", () => {
    const result = validateAlertDraft(draft({ threshold: " 0.25 " }));
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.body).toEqual({
        name: "errors",
        type: "error_rate",
        threshold: 0.25,
        config: "",
        windowSecs: 3600,
        settleSecs: 300,
        webhookUrl: "https://hooks.example.test/services/abc",
      });
    }
  });

  it("refuses a percentage threshold before it reaches the API", () => {
    const result = validateAlertDraft(draft({ threshold: "10%" }));
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toContain("0.1");
  });

  it("refuses blank required fields", () => {
    expect(validateAlertDraft(draft({ name: "  " })).ok).toBe(false);
    expect(validateAlertDraft(draft({ webhookUrl: "" })).ok).toBe(false);
  });

  it("requires an assertion name for a fail-rate rule", () => {
    const result = validateAlertDraft(
      draft({ type: "assertion_fail_rate", config: "" }),
    );
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toContain("assertion");
  });

  it("carries the assertion name only for a fail-rate rule", () => {
    const failRate = validateAlertDraft(
      draft({ type: "assertion_fail_rate", config: "polite" }),
    );
    expect(failRate.ok).toBe(true);
    if (failRate.ok) expect(failRate.body.config).toBe("polite");

    const errorRate = validateAlertDraft(draft({ config: "ignored" }));
    expect(errorRate.ok).toBe(true);
    if (errorRate.ok) expect(errorRate.body.config).toBe("");
  });

  it("preserves the project when editing an existing rule", () => {
    const result = validateAlertDraft(draft({ project: "prod" }));
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.body.project).toBe("prod");
  });

  it("never emits a non-finite threshold", () => {
    for (const text of ["", "10%", "abc", "Infinity", "1e999"]) {
      const result = validateAlertDraft(draft({ threshold: text }));
      expect(result.ok).toBe(false);
    }
  });
});

describe("draftFromAlert", () => {
  it("round-trips a stored rule into the form", () => {
    expect(draftFromAlert(alert)).toEqual({
      project: "default",
      name: "errors",
      type: "error_rate",
      threshold: "0.1",
      config: "",
      windowSecs: 3600,
      settleSecs: 300,
      webhookUrl: "",
    });
  });
});

describe("alert display helpers", () => {
  it("renders the configured assertion and threshold", () => {
    expect(alertCondition(alert)).toBe("error_rate > 0.1");
    expect(
      alertCondition({ ...alert, type: "assertion_fail_rate", config: "polite" }),
    ).toBe("assertion_fail_rate (polite) > 0.1");
  });

  it("shortens a webhook to its host and path hint", () => {
    expect(shortWebhook("https://hooks.slack.com/services/abc")).toBe(
      "hooks.slack.com/services/abc",
    );
    expect(
      shortWebhook("https://hooks.example.test/very/long/path/that/continues"),
    ).toBe("hooks.example.test/very/long…");
  });
});
