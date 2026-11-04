export type RunStatus = "running" | "ok" | "error";

export type Run = {
  id: string;
  project: string;
  service: string;
  agentName: string;
  status: RunStatus;
  start: string;
  durationMs: number;
  inputTokens: number;
  outputTokens: number;
  llmCalls: number;
  toolCalls: number;
  models: string;
  prompts?: string;
  costUsd?: number;
  costPartial?: boolean;
  error: string;
};

export type Message = { role: string; content: string };

export type Step = {
  id: string;
  parentId: string;
  kind: "agent" | "llm" | "tool" | "generic";
  name: string;
  status: RunStatus;
  start: string;
  offsetMs: number;
  durationMs: number;
  error?: string;
  llm?: {
    provider: string;
    requestModel: string;
    responseModel: string;
    inputTokens: number;
    outputTokens: number;
    cacheReadTokens: number;
    reasoningTokens: number;
    costUsd?: number;
    inputMessages?: Message[];
    outputMessages?: Message[];
  };
  tool?: {
    name: string;
    callId: string;
    arguments?: string;
    result?: string;
  };
};

export type AssertionResult = {
  assertionId: number;
  name: string;
  type: string;
  pass: boolean;
  detail: string;
};

export type RunDetail = {
  run: Run;
  steps: Step[];
  assertionResults?: AssertionResult[];
};

export type AssertionRate = {
  assertionId: number;
  name: string;
  passed: number;
  total: number;
};

export type Stats = {
  runs: number;
  errors: number;
  avgDurationMs: number;
  p50DurationMs: number;
  p95DurationMs: number;
  totalCostUsd?: number;
  avgCostUsd?: number;
  inputTokens: number;
  outputTokens: number;
  assertionRates: AssertionRate[];
};

export type SavedView = {
  id: number;
  name: string;
  params: unknown;
};

export type Share = { token: string; createdAt: string };

export type Alert = {
  id: number;
  project: string;
  name: string;
  type: string;
  threshold: number;
  windowSecs: number;
  config: string;
  webhookUrl: string;
  enabled: boolean;
  firing: boolean;
  settleSecs: number;
  pendingSinceNs?: number;
};

export type AuditEntry = {
  at: string;
  action: string;
  entity: string;
  detail: string;
};

export type Health = { status: string; version: string };

export type AlertInput = {
  project?: string;
  name: string;
  type: string;
  threshold: number;
  config: string;
  windowSecs: number;
  settleSecs: number;
  webhookUrl: string;
};

export type RunWire = Run;
export type StepWire = Step;
export type AssertionResultWire = AssertionResult;
export type AssertionRateWire = AssertionRate;
export type SavedViewWire = SavedView;
export type ShareWire = Share;
export type AlertWire = Alert;
export type AuditEntryWire = AuditEntry;
export type HealthWire = Health;

export type RunsWire = { runs?: RunWire[] };
export type RunDetailWire = {
  run: RunWire;
  steps?: StepWire[];
  assertionResults?: AssertionResultWire[];
};
export type StatsWire = {
  runs: number;
  errors: number;
  avgDurationMs: number;
  p50DurationMs: number;
  p95DurationMs: number;
  totalCostUsd?: number;
  avgCostUsd?: number;
  inputTokens: number;
  outputTokens: number;
  assertionRates?: AssertionRateWire[];
};
export type ViewsWire = { views?: SavedViewWire[] };
export type SharesWire = { shares?: ShareWire[] };
export type AlertsWire = { alerts?: AlertWire[] };
export type AuditWire = { entries?: AuditEntryWire[] };
export type ShareMintWire = { token: string; url: string };
export type SharedRunWire = RunDetailWire;

export type ShareMint = { token: string; url: string };

export const decodeRun = (wire: RunWire): Run => wire;

export const decodeRunList = (wire: RunsWire): Run[] =>
  (wire.runs ?? []).map(decodeRun);

export const decodeRunDetail = (wire: RunDetailWire): RunDetail => ({
  run: decodeRun(wire.run),
  steps: wire.steps ?? [],
  assertionResults: wire.assertionResults ?? [],
});

export const decodeStats = (wire: StatsWire): Stats => ({
  ...wire,
  assertionRates: wire.assertionRates ?? [],
});

export const decodeSavedView = (wire: SavedViewWire): SavedView => wire;

export const decodeViews = (wire: ViewsWire): SavedView[] =>
  (wire.views ?? []).map(decodeSavedView);

export const decodeShare = (wire: ShareWire): Share => wire;

export const decodeShares = (wire: SharesWire): Share[] =>
  (wire.shares ?? []).map(decodeShare);

export const decodeAlert = (wire: AlertWire): Alert => wire;

export const decodeAlerts = (wire: AlertsWire): Alert[] =>
  (wire.alerts ?? []).map(decodeAlert);

export const decodeAuditEntries = (wire: AuditWire): AuditEntry[] =>
  wire.entries ?? [];

export const decodeHealth = (wire: HealthWire): Health => wire;

export const decodeShareMint = (wire: ShareMintWire): ShareMint => wire;

export const endpoints = {
  runs: (query: string) => `/api/runs?${query}`,
  runsCsv: (query: string) => `/api/runs.csv?${query}`,
  run: (id: string) => `/api/runs/${encodeURIComponent(id)}`,
  runShares: (id: string) => `/api/runs/${encodeURIComponent(id)}/shares`,
  mintShare: (id: string) => `/api/runs/${encodeURIComponent(id)}/share`,
  revokeShare: (token: string) =>
    `/api/shares/${encodeURIComponent(token)}`,
  sharedRun: (token: string) =>
    `/api/shared/${encodeURIComponent(token)}`,
  stats: (query: string) => `/api/stats?${query}`,
  views: () => "/api/views",
  view: (id: number) => `/api/views/${id}`,
  alerts: () => "/api/alerts",
  alert: (id: number) => `/api/alerts/${id}`,
  audit: (limit: number) => `/api/audit?limit=${limit}`,
  health: () => "/healthz",
};

export const csvTruncatedHeader = "X-Otterscope-Truncated";
export const csvRowLimitHeader = "X-Otterscope-Row-Limit";
