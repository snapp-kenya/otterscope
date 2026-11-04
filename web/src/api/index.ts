import {
  decodeAlert,
  decodeAlerts,
  decodeAssertions,
  decodeAuditEntries,
  decodeCreatedToken,
  decodeEvaluateReport,
  decodeHealth,
  decodeRunDetail,
  decodeRunList,
  decodeSavedView,
  decodeShareMint,
  decodeShares,
  decodeSharedRun,
  decodeStats,
  decodeTokens,
  decodeViews,
  endpoints,
  evaluateQuery,
  type AlertInput,
  type AlertsWire,
  type AlertWire,
  type AssertionInput,
  type AssertionsWire,
  type AssertionWire,
  type AuditWire,
  type CreatedToken,
  type CreatedTokenWire,
  type EvaluateOptions,
  type EvaluateReport,
  type EvaluateReportWire,
  type HealthWire,
  type Run,
  type RunDetail,
  type RunDetailWire,
  type RunsWire,
  type SavedViewWire,
  type ShareMintWire,
  type SharesWire,
  type SharedRunWire,
  type StatsWire,
  type Token,
  type TokenInput,
  type TokensWire,
  type ViewsWire,
} from "@/api/contract";
import {
  adminDelete,
  adminPostJson,
  deleteResource,
  getJson,
  postJson,
  request,
} from "@/api/client";

export * from "@/api/contract";
export {
  ApiError,
  apiFetch,
  csvTruncationNotice,
  errorMessage,
  failureMessage,
  getJson,
  readToken,
  request,
  transportMessage,
} from "@/api/client";

export const fetchRuns = async (
  query: string,
  signal?: AbortSignal,
): Promise<Run[]> =>
  decodeRunList(await getJson<RunsWire>(endpoints.runs(query), signal));

export const fetchRun = async (
  id: string,
  signal?: AbortSignal,
): Promise<RunDetail> =>
  decodeRunDetail(await getJson<RunDetailWire>(endpoints.run(id), signal));

export const fetchSharedRun = async (
  token: string,
  signal?: AbortSignal,
): Promise<RunDetail> =>
  decodeSharedRun(
    await getJson<SharedRunWire>(endpoints.sharedRun(token), signal),
  );

export const fetchStats = async (
  query: string,
  signal?: AbortSignal,
) => decodeStats(await getJson<StatsWire>(endpoints.stats(query), signal));

export const fetchAssertions = async (
  project?: string,
  signal?: AbortSignal,
) =>
  decodeAssertions(
    await getJson<AssertionsWire>(endpoints.assertions(project), signal),
  );

export const createAssertion = async (input: AssertionInput) =>
  await postJson<AssertionWire>(endpoints.assertions(), input);

export const deleteAssertion = async (id: number): Promise<void> =>
  deleteResource(endpoints.assertion(id));

export const evaluateAssertions = async (
  options: EvaluateOptions,
): Promise<EvaluateReport> =>
  decodeEvaluateReport(
    await postJson<EvaluateReportWire>(
      endpoints.evaluate(evaluateQuery(options)),
    ),
  );

export const fetchViews = async (signal?: AbortSignal) =>
  decodeViews(await getJson<ViewsWire>(endpoints.views(), signal));

export const createView = async (name: string, params: unknown) =>
  decodeSavedView(
    await postJson<SavedViewWire>(endpoints.views(), { name, params }),
  );

export const deleteView = async (id: number): Promise<void> =>
  deleteResource(endpoints.view(id));

export const fetchShares = async (id: string, signal?: AbortSignal) =>
  decodeShares(
    await getJson<SharesWire>(endpoints.runShares(id), signal),
  );

export const mintShare = async (id: string) =>
  decodeShareMint(
    await adminPostJson<ShareMintWire>(endpoints.mintShare(id)),
  );

export const revokeShare = async (token: string): Promise<void> =>
  adminDelete(endpoints.revokeShare(token));

export const fetchAlerts = async (signal?: AbortSignal) =>
  decodeAlerts(await getJson<AlertsWire>(endpoints.alerts(), signal));

export const createAlert = async (input: AlertInput) =>
  decodeAlert(await postJson<AlertWire>(endpoints.alerts(), input));

export const deleteAlert = async (id: number): Promise<void> =>
  deleteResource(endpoints.alert(id));

export const fetchTokens = async (signal?: AbortSignal): Promise<Token[]> =>
  decodeTokens(await getJson<TokensWire>(endpoints.tokens(), signal));

export const createToken = async (
  input: TokenInput,
): Promise<CreatedToken> =>
  decodeCreatedToken(
    await postJson<CreatedTokenWire>(endpoints.tokens(), input),
  );

export const deleteToken = async (id: string): Promise<void> =>
  deleteResource(endpoints.token(id));

export const fetchAudit = async (
  limit = 500,
  signal?: AbortSignal,
) =>
  decodeAuditEntries(
    await getJson<AuditWire>(endpoints.audit(limit), signal),
  );

export const fetchHealth = async (signal?: AbortSignal) =>
  decodeHealth(await getJson<HealthWire>(endpoints.health(), signal));

export const fetchMetricsText = async (signal?: AbortSignal) =>
  (await request(endpoints.metrics(), { signal })).text();
