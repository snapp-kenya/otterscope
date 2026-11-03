// The whole app is served under a configurable prefix (`serve -base-path`,
// e.g. /otterscope) and the bundle is compiled with the matching Vite `base`.
// Every URL the UI fetches, links to, or pushes into history therefore has to
// go through these helpers: a hard-coded "/api/runs" would hit the ingress root
// instead of this app.
//
// Vite's BASE_URL always carries a trailing slash ("/" at the root,
// "/otterscope/" under a prefix); BASE_PATH is that value in the server's
// spelling, so joining is plain concatenation.
export const BASE_PATH = normalizeBase(import.meta.env.BASE_URL);

/** normalizeBase drops surrounding slashes: "/otterscope/" -> "/otterscope". */
export function normalizeBase(raw: string | undefined): string {
  const p = (raw ?? "").trim().replace(/^\/+|\/+$/g, "");
  return p ? "/" + p : "";
}

/** withBase prefixes a root-absolute app path: "/api/runs" -> "/otterscope/api/runs". */
export function withBase(path: string, base: string = BASE_PATH): string {
  if (base && (path === base || path.startsWith(base + "/"))) return path;
  return base + path;
}

/** stripBase maps a location pathname back to an app path ("/" at the mount point). */
export function stripBase(pathname: string, base: string = BASE_PATH): string {
  if (!base) return pathname;
  if (pathname === base) return "/";
  if (pathname.startsWith(base + "/")) return pathname.slice(base.length);
  return pathname;
}
