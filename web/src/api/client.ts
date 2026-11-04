import { withBase } from "@/lib/base";
import {
  csvRowLimitHeader,
  csvTruncatedHeader,
} from "@/api/contract";

export class ApiError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export const csvTruncationNotice = (res: Response): string | null => {
  if (res.headers.get(csvTruncatedHeader) !== "true") return null;
  const limit = res.headers.get(csvRowLimitHeader) ?? "the export limit";
  return `More runs matched than could be exported — this file holds the newest ${limit}. Narrow the filters for the rest.`;
};

export const failureMessage = async (res: Response): Promise<string> => {
  let detail = "";
  try {
    const body: unknown = await res.json();
    if (
      typeof body === "object" &&
      body !== null &&
      "error" in body &&
      typeof body.error === "string"
    ) {
      detail = body.error;
    }
  } catch {
    detail = "";
  }
  const status = res.statusText
    ? `${res.status} ${res.statusText}`
    : String(res.status);
  return detail ? `${status}: ${detail}` : status;
};

export const transportMessage = (err: unknown): string => {
  const detail = err instanceof Error ? err.message : String(err ?? "");
  return detail
    ? `Could not reach the server (${detail})`
    : "Could not reach the server";
};

const TOKEN_KEY = "otterscope_token";

export const readToken = (): string =>
  localStorage.getItem(TOKEN_KEY) ?? "";

type AuthState = { readRetried: boolean; adminRetried: boolean };

const storePromptedToken = (message: string): string => {
  const entered = window.prompt(message)?.trim() ?? "";
  if (entered) localStorage.setItem(TOKEN_KEY, entered);
  return entered;
};

const adminTokenRequired = async (res: Response): Promise<boolean> => {
  try {
    const body: unknown = await res.clone().json();
    return (
      typeof body === "object" &&
      body !== null &&
      "error" in body &&
      body.error === "admin token required"
    );
  } catch {
    return false;
  }
};

const fetchWithAuth = async (
  url: string,
  opts: RequestInit,
  state: AuthState,
  adminRetry: boolean,
): Promise<Response> => {
  const token = readToken();
  const headers = new Headers(opts.headers ?? {});
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const res = await fetch(url, { ...opts, headers });
  if (res.status === 401 && !state.readRetried && !state.adminRetried) {
    const entered = storePromptedToken(
      "This Otterscope instance requires a read token:",
    );
    if (entered) {
      return fetchWithAuth(
        url,
        opts,
        { ...state, readRetried: true },
        adminRetry,
      );
    }
  }
  if (
    res.status === 403 &&
    adminRetry &&
    !state.adminRetried &&
    (await adminTokenRequired(res))
  ) {
    const entered = storePromptedToken(
      "This action needs an admin token:",
    );
    if (entered) {
      return fetchWithAuth(
        url,
        opts,
        { ...state, adminRetried: true },
        adminRetry,
      );
    }
  }
  return res;
};

export const apiFetch = (
  url: string,
  opts: RequestInit = {},
): Promise<Response> =>
  fetchWithAuth(url, opts, { readRetried: false, adminRetried: false }, false);

const jsonHeaders = { "Content-Type": "application/json" };

export const request = async (
  path: string,
  init?: RequestInit,
): Promise<Response> => {
  const res = await apiFetch(withBase(path), init);
  if (!res.ok) throw new ApiError(await failureMessage(res), res.status);
  return res;
};

export const getJson = async <T>(
  path: string,
  signal?: AbortSignal,
): Promise<T> => {
  const res = await request(path, { signal });
  return (await res.json()) as T;
};

export const postJson = async <T>(
  path: string,
  body?: unknown,
): Promise<T> => {
  const res = await request(path, {
    method: "POST",
    headers: body === undefined ? undefined : jsonHeaders,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return (await res.json()) as T;
};

export const deleteResource = async (path: string): Promise<void> => {
  await request(path, { method: "DELETE" });
};

export const errorMessage = (err: unknown): string =>
  err instanceof ApiError ? err.message : transportMessage(err);

const adminFetch = async (
  path: string,
  init: RequestInit,
): Promise<Response> => {
  const res = await fetchWithAuth(
    withBase(path),
    init,
    { readRetried: false, adminRetried: false },
    true,
  );
  if (!res.ok) throw new ApiError(await failureMessage(res), res.status);
  return res;
};

export const adminPostJson = async <T>(
  path: string,
  body?: unknown,
): Promise<T> => {
  const res = await adminFetch(path, {
    method: "POST",
    headers: body === undefined ? undefined : jsonHeaders,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return (await res.json()) as T;
};

export const adminDelete = async (path: string): Promise<void> => {
  await adminFetch(path, { method: "DELETE" });
};
