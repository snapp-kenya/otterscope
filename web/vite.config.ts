/// <reference types="vite/client" />
import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const normalizeBase = (raw: string | undefined) => {
  const prefix = (raw ?? "").trim().replace(/^\/+|\/+$/g, "");
  return prefix ? `/${prefix}/` : "/";
};

const base = normalizeBase(process.env.OTTERSCOPE_BASE_PATH);
const proxyTarget = "http://localhost:8317";
const mount = base.replace(/\/$/, "");

const proxy = Object.fromEntries(
  ["/api", "/healthz"].map((path) => [
    mount + path,
    { target: proxyTarget, rewrite: (url: string) => url.slice(mount.length) },
  ]),
);

export default defineConfig({
  base,
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  server: { proxy },
});
