export const runStatuses = ["running", "ok", "error"] as const;
export type RunStatus = (typeof runStatuses)[number];

export const stepKinds = ["agent", "llm", "tool", "generic"] as const;
export type StepKind = (typeof stepKinds)[number];

export const assertionTypes = [
  "contains",
  "not_contains",
  "regex",
  "is_json",
  "max_latency_ms",
  "max_cost_usd",
  "llm_judge",
] as const;
export type AssertionType = (typeof assertionTypes)[number];

export type ErrorEnvelope = { error: string };

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

export type LlmCall = {
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

export type ToolCall = {
  name: string;
  callId: string;
  arguments?: string;
  result?: string;
};

export type Step = {
  id: string;
  parentId: string;
  kind: StepKind;
  name: string;
  status: RunStatus;
  start: string;
  offsetMs: number;
  durationMs: number;
  error?: string;
  llm?: LlmCall;
  tool?: ToolCall;
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
  assertionResults: AssertionResult[];
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

export type Assertion = {
  id: number;
  project: string;
  name: string;
  type: AssertionType;
  config: string;
  enabled: boolean;
};

export type AssertionInput = {
  project?: string;
  name: string;
  type: AssertionType;
  config: string;
};

export type EvaluateReport = {
  runsEvaluated: number;
  runsSkipped: number;
  runLimit: number;
  judgeCalls: number;
  judgeCallsSkipped: number;
  judgeCallLimit: number;
};

export type EvaluateOptions = {
  project?: string;
  run?: string;
  since?: string;
  until?: string;
  limit?: number;
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

export type AlertInput = {
  project?: string;
  name: string;
  type: string;
  threshold: number;
  windowSecs?: number;
  config?: string;
  webhookUrl: string;
  settleSecs?: number;
};

export type TokenScope = "read" | "admin";

export type Token = {
  id: string;
  name: string;
  scope: TokenScope;
  createdAt: string;
  lastUsedAt?: string;
};

export type CreatedToken = Token & { token: string };

export type TokenInput = { name: string; scope?: TokenScope };

export type AuditEntry = {
  at: string;
  action: string;
  entity: string;
  detail: string;
};

export type Health = { status: string; version: string };

export type RunWire = Run;
export type StepWire = Step;
export type LlmWire = LlmCall;
export type ToolWire = ToolCall;
export type AssertionResultWire = AssertionResult;
export type AssertionRateWire = AssertionRate;
export type AssertionWire = Assertion;
export type SavedViewWire = SavedView;
export type ShareWire = Share;
export type AlertWire = Alert;
export type TokenWire = Token;
export type CreatedTokenWire = CreatedToken;
export type AuditEntryWire = AuditEntry;
export type HealthWire = Health;
export type EvaluateReportWire = EvaluateReport;

export type RunsWire = { runs: RunWire[] };
export type RunDetailWire = {
  run: RunWire;
  steps: StepWire[];
  assertionResults: AssertionResultWire[];
};
export type SharedRunWire = { run: RunWire; steps: StepWire[] };
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
  assertionRates: AssertionRateWire[];
};
export type AssertionsWire = { assertions: AssertionWire[] };
export type ViewsWire = { views: SavedViewWire[] };
export type SharesWire = { shares: ShareWire[] };
export type AlertsWire = { alerts: AlertWire[] };
export type TokensWire = { tokens: TokenWire[] };
export type AuditWire = { entries: AuditEntryWire[] };
export type ShareMintWire = { token: string; url: string };

export type ShareMint = { token: string; url: string };

export const decodeRun = (wire: RunWire): Run => wire;

export const decodeRunList = (wire: RunsWire): Run[] =>
  (wire.runs ?? []).map(decodeRun);

export const decodeRunDetail = (wire: RunDetailWire): RunDetail => ({
  run: decodeRun(wire.run),
  steps: wire.steps ?? [],
  assertionResults: wire.assertionResults ?? [],
});

export const decodeSharedRun = (wire: SharedRunWire): RunDetail => ({
  run: decodeRun(wire.run),
  steps: wire.steps ?? [],
  assertionResults: [],
});

export const decodeStats = (wire: StatsWire): Stats => ({
  ...wire,
  assertionRates: wire.assertionRates ?? [],
});

export const decodeAssertion = (wire: AssertionWire): Assertion => wire;

export const decodeAssertions = (wire: AssertionsWire): Assertion[] =>
  (wire.assertions ?? []).map(decodeAssertion);

export const decodeEvaluateReport = (
  wire: EvaluateReportWire,
): EvaluateReport => wire;

export const decodeSavedView = (wire: SavedViewWire): SavedView => wire;

export const decodeViews = (wire: ViewsWire): SavedView[] =>
  (wire.views ?? []).map(decodeSavedView);

export const decodeShare = (wire: ShareWire): Share => wire;

export const decodeShares = (wire: SharesWire): Share[] =>
  (wire.shares ?? []).map(decodeShare);

export const decodeAlert = (wire: AlertWire): Alert => wire;

export const decodeAlerts = (wire: AlertsWire): Alert[] =>
  (wire.alerts ?? []).map(decodeAlert);

export const decodeToken = (wire: TokenWire): Token => wire;

export const decodeTokens = (wire: TokensWire): Token[] =>
  (wire.tokens ?? []).map(decodeToken);

export const decodeCreatedToken = (wire: CreatedTokenWire): CreatedToken =>
  wire;

export const decodeAuditEntries = (wire: AuditWire): AuditEntry[] =>
  wire.entries ?? [];

export const decodeHealth = (wire: HealthWire): Health => wire;

export const decodeShareMint = (wire: ShareMintWire): ShareMint => wire;

export const evaluateQuery = (options: EvaluateOptions): string => {
  const query = new URLSearchParams();
  if (options.project) query.set("project", options.project);
  if (options.run) query.set("run", options.run);
  if (options.since) query.set("since", options.since);
  if (options.until) query.set("until", options.until);
  if (options.limit !== undefined) query.set("limit", String(options.limit));
  return query.toString();
};

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
  assertions: (project?: string) =>
    project
      ? `/api/assertions?project=${encodeURIComponent(project)}`
      : "/api/assertions",
  assertion: (id: number) => `/api/assertions/${id}`,
  evaluate: (query: string) => `/api/assertions/evaluate?${query}`,
  views: () => "/api/views",
  view: (id: number) => `/api/views/${id}`,
  alerts: () => "/api/alerts",
  alert: (id: number) => `/api/alerts/${id}`,
  tokens: () => "/api/tokens",
  token: (id: string) => `/api/tokens/${encodeURIComponent(id)}`,
  audit: (limit: number) => `/api/audit?limit=${limit}`,
  health: () => "/healthz",
  metrics: () => "/metrics",
  mcp: () => "/mcp",
};

export const csvTruncatedHeader = "X-Otterscope-Truncated";
export const csvRowLimitHeader = "X-Otterscope-Row-Limit";
