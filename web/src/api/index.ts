import {
  decodeAlert,
  decodeAlerts,
  decodeAuditEntries,
  decodeHealth,
  decodeRunDetail,
  decodeShareMint,
  decodeRunList,
  decodeSavedView,
  decodeShares,
  decodeStats,
  decodeViews,
  endpoints,
  type AlertsWire,
  type AlertInput,
  type AlertWire,
  type AuditWire,
  type HealthWire,
  type Run,
  type RunDetailWire,
  type RunsWire,
  type SavedViewWire,
  type ShareMintWire,
  type SharesWire,
  type SharedRunWire,
  type StatsWire,
  type ViewsWire,
} from "@/api/contract";
import { deleteResource, getJson, postJson } from "@/api/client";

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
) =>
  decodeRunDetail(await getJson<RunDetailWire>(endpoints.run(id), signal));

export const fetchSharedRun = async (
  token: string,
  signal?: AbortSignal,
) =>
  decodeRunDetail(
    await getJson<SharedRunWire>(endpoints.sharedRun(token), signal),
  );

export const fetchStats = async (
  query: string,
  signal?: AbortSignal,
) => decodeStats(await getJson<StatsWire>(endpoints.stats(query), signal));

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
  decodeShareMint(await postJson<ShareMintWire>(endpoints.mintShare(id)));

export const revokeShare = async (token: string): Promise<void> =>
  deleteResource(endpoints.revokeShare(token));

export const fetchAlerts = async (signal?: AbortSignal) =>
  decodeAlerts(await getJson<AlertsWire>(endpoints.alerts(), signal));

export const createAlert = async (input: AlertInput) =>
  decodeAlert(await postJson<AlertWire>(endpoints.alerts(), input));

export const deleteAlert = async (id: number): Promise<void> =>
  deleteResource(endpoints.alert(id));

export const fetchAudit = async (
  limit = 500,
  signal?: AbortSignal,
) =>
  decodeAuditEntries(
    await getJson<AuditWire>(endpoints.audit(limit), signal),
  );

export const fetchHealth = async (signal?: AbortSignal) =>
  decodeHealth(await getJson<HealthWire>(endpoints.health(), signal));
