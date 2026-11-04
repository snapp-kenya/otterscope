/// <reference types="vite/client" />
import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

const normalizeBase = (raw: string | undefined) => {
  const prefix = (raw ?? "").trim().replace(/^\/+|\/+$/g, "");
  return prefix ? `/${prefix}/` : "/";
};

const base = normalizeBase(process.env.OTTERSCOPE_BASE_PATH);

// Records the base baked into the bundle so the server can warn when it is
// started with a different one: a root-built image under -base-path otherwise
// renders a UI whose router and API calls point at the root.
const basePathMarker = (bakedBase: string): Plugin => ({
  name: "otterscope-base-path-marker",
  transformIndexHtml: () => [
    {
      tag: "meta",
      attrs: {
        name: "otterscope-base-path",
        content: bakedBase.replace(/\/$/, ""),
      },
      injectTo: "head-prepend",
    },
  ],
});

const proxyTarget = "http://localhost:80";
const mount = base.replace(/\/$/, "");

const proxy = Object.fromEntries(
  ["/api", "/healthz"].map((path) => [
    mount + path,
    { target: proxyTarget, rewrite: (url: string) => url.slice(mount.length) },
  ]),
);

export default defineConfig({
  base,
  plugins: [basePathMarker(base), react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  server: { proxy },
});
