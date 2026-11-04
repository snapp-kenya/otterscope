export const normalizeBase = (raw: string | undefined): string => {
  const prefix = (raw ?? "").trim().replace(/^\/+|\/+$/g, "");
  return prefix ? `/${prefix}` : "";
};

export const BASE_PATH = normalizeBase(import.meta.env.BASE_URL);

export const withBase = (path: string, base: string = BASE_PATH): string => {
  if (base && (path === base || path.startsWith(`${base}/`))) return path;
  return base + path;
};
