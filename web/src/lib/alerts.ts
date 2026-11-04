import type { Alert, AlertInput } from "@/api";
import { parseThreshold, type ThresholdResult } from "@/lib/alertForm";

export type AlertDraft = {
  project: string;
  name: string;
  type: string;
  threshold: string;
  config: string;
  windowSecs: number;
  settleSecs: number;
  webhookUrl: string;
};

export type AlertTypeOption = {
  value: string;
  label: string;
  hint: string;
};

export const alertTypes: AlertTypeOption[] = [
  {
    value: "error_rate",
    label: "error rate >",
    hint: "fraction 0–1 (e.g. 0.1 = 10%)",
  },
  { value: "cost", label: "window cost > $", hint: "USD total in window" },
  {
    value: "p95_latency",
    label: "p95 latency >",
    hint: "milliseconds",
  },
  {
    value: "assertion_fail_rate",
    label: "assertion fail rate >",
    hint: "fraction 0–1; set assertion name",
  },
];

export const alertWindows = [
  { value: 900, label: "15 min" },
  { value: 3600, label: "1 hour" },
  { value: 86400, label: "24 hours" },
];

export const alertSettles = [
  { value: 0, label: "notify immediately" },
  { value: 300, label: "after 5 min" },
  { value: 900, label: "after 15 min" },
  { value: 3600, label: "after 1 hour" },
];

export const alertTypeOption = (type: string): AlertTypeOption =>
  alertTypes.find((option) => option.value === type) ?? alertTypes[0]!;

export const alertTypeHint = (type: string): string =>
  alertTypeOption(type).hint;

export const emptyAlertDraft = (): AlertDraft => ({
  project: "",
  name: "",
  type: "error_rate",
  threshold: "0.1",
  config: "",
  windowSecs: 3600,
  settleSecs: 300,
  webhookUrl: "",
});

export const draftFromAlert = (alert: Alert): AlertDraft => ({
  project: alert.project,
  name: alert.name,
  type: alert.type,
  threshold: String(alert.threshold),
  config: alert.config,
  windowSecs: alert.windowSecs,
  settleSecs: alert.settleSecs,
  webhookUrl: "",
});

export const validateAlertDraft = (
  draft: AlertDraft,
): { ok: true; body: AlertInput } | { ok: false; error: string } => {
  const name = draft.name.trim();
  if (!name) return { ok: false, error: "Give the alert a name." };
  const webhookUrl = draft.webhookUrl.trim();
  if (!webhookUrl) {
    return { ok: false, error: "Add a webhook URL to notify." };
  }
  const config = draft.config.trim();
  if (draft.type === "assertion_fail_rate" && !config) {
    return { ok: false, error: "Name the assertion to watch." };
  }
  const parsed: ThresholdResult = parseThreshold(
    draft.threshold,
    alertTypeHint(draft.type),
  );
  if (!parsed.ok) return parsed;
  const body: AlertInput = {
    name,
    type: draft.type,
    threshold: parsed.value,
    config: draft.type === "assertion_fail_rate" ? config : "",
    windowSecs: draft.windowSecs,
    settleSecs: draft.settleSecs,
    webhookUrl,
  };
  if (draft.project) body.project = draft.project;
  return { ok: true, body };
};

export const alertCondition = (alert: Alert): string => {
  const config = alert.config ? ` (${alert.config})` : "";
  return `${alert.type}${config} > ${alert.threshold}`;
};

export const shortWebhook = (url: string): string => {
  const bare = url.replace(/^https?:\/\//, "");
  return bare.length > 28 ? `${bare.slice(0, 28)}…` : bare;
};
