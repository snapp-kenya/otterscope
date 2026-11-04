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

export const apiFetch = async (
  url: string,
  opts: RequestInit = {},
  retried = false,
): Promise<Response> => {
  const token = readToken();
  const headers = new Headers(opts.headers ?? {});
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const res = await fetch(url, { ...opts, headers });
  if (res.status === 401 && !retried) {
    const entered = window.prompt(
      "This Otterscope instance requires a read token:",
    );
    if (entered) {
      localStorage.setItem(TOKEN_KEY, entered.trim());
      return apiFetch(url, opts, true);
    }
  }
  return res;
};

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
