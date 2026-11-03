/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The UI is served under a path prefix by the Go server (`-base-path` /
// OTTERSCOPE_BASE_PATH, e.g. /otterscope). Building with the same value makes
// index.html's asset URLs resolve under the prefix instead of at the ingress
// root. Vite wants a leading and trailing slash; the server's flag wants
// neither, so each side normalizes its own.
const base = normalizeBase(process.env.OTTERSCOPE_BASE_PATH);
const proxyTarget = "http://localhost:8317";
const mount = base.replace(/\/$/, ""); // "" at the root

// Dev server proxies API + health to a locally running `otterscope serve` so
// `npm run dev` gives hot reload against real data. The proxy keys carry the
// prefix (the dev server serves the UI under it too) and rewrite strips it,
// because the Go server being proxied is normally still mounted at root.
const proxy = Object.fromEntries(
  ["/api", "/healthz"].map((p) => [
    mount + p,
    { target: proxyTarget, rewrite: (path: string) => path.slice(mount.length) },
  ]),
);

function normalizeBase(raw: string | undefined): string {
  const p = (raw ?? "").trim().replace(/^\/+|\/+$/g, "");
  return p ? `/${p}/` : "/";
}

export default defineConfig({
  base,
  plugins: [react()],
  server: { proxy },
  // happy-dom, because the units under test read window.location, history
  // and localStorage — the browser surface is part of what they do.
  //
  // happy-dom rather than jsdom for its Node range: jsdom 30 declares
  // ^22.22.2 || ^24.15.0 || >=26.0.0, which excludes all of Node 20 and 23
  // and most of 24, so a contributor on Node 20 LTS cannot run the tests at
  // all. happy-dom needs >=20, matching the engines field. It is also about
  // 2.5x faster to set up.
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts"],
  },
});
