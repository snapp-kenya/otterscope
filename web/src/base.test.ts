import { describe, expect, it } from "vitest";
import { BASE_PATH, normalizeBase, stripBase, withBase } from "./base";

// The UI is served under a configurable prefix (`serve -base-path`, e.g.
// /otterscope). These helpers are the only place that prefix is applied, so
// they are tested at both mount points: "" (the default deployment) and
// "/otterscope" (behind path-based ingress routing).

describe("normalizeBase", () => {
  it("is empty at the root", () => {
    expect(normalizeBase("/")).toBe("");
    expect(normalizeBase("")).toBe("");
    expect(normalizeBase(undefined)).toBe("");
  });

  it("accepts a prefix written with or without surrounding slashes", () => {
    expect(normalizeBase("/otterscope/")).toBe("/otterscope");
    expect(normalizeBase("otterscope")).toBe("/otterscope");
    expect(normalizeBase("  /otterscope  ")).toBe("/otterscope");
  });

  it("agrees with Vite's BASE_URL", () => {
    // Vite always hands over a trailing slash; BASE_PATH is the server's shape.
    expect(BASE_PATH).toBe(normalizeBase(import.meta.env.BASE_URL));
  });
});

describe("withBase", () => {
  it("leaves app paths alone at the root", () => {
    expect(withBase("/api/runs")).toBe("/api/runs");
    expect(withBase("/")).toBe("/");
  });

  it("prefixes API calls, assets and links under a base path", () => {
    expect(withBase("/api/runs", "/otterscope")).toBe("/otterscope/api/runs");
    expect(withBase("/api/runs?limit=100", "/otterscope")).toBe(
      "/otterscope/api/runs?limit=100",
    );
    expect(withBase("/compare", "/otterscope")).toBe("/otterscope/compare");
  });

  it("maps the app root to the mount point", () => {
    expect(withBase("/", "/otterscope")).toBe("/otterscope/");
  });

  it("does not double-prefix a path that is already mounted", () => {
    expect(withBase("/otterscope/api/runs", "/otterscope")).toBe("/otterscope/api/runs");
    expect(withBase("/otterscope/api/runs?limit=1", "/otterscope")).toBe(
      "/otterscope/api/runs?limit=1",
    );
    // Only a full path segment counts as "already mounted": a sibling path
    // that merely starts with the same characters is outside the app and
    // still needs the prefix.
    expect(withBase("/otterscopeish/api", "/otterscope")).toBe("/otterscope/otterscopeish/api");
  });
});

describe("stripBase", () => {
  it("returns the app root for the mount point itself", () => {
    expect(stripBase("/otterscope", "/otterscope")).toBe("/");
    expect(stripBase("/otterscope/", "/otterscope")).toBe("/");
  });

  it("strips the prefix from a nested route, leaving the query alone", () => {
    expect(stripBase("/otterscope/runs/abc", "/otterscope")).toBe("/runs/abc");
    // Callers hand over whatever they have; a query on the pathname must not
    // be mistaken for part of the mount.
    expect(stripBase("/otterscope/runs?limit=2", "/otterscope")).toBe("/runs?limit=2");
  });

  it("leaves a path outside the prefix untouched, so routing can reject it", () => {
    expect(stripBase("/elsewhere", "/otterscope")).toBe("/elsewhere");
    expect(stripBase("/otterscopeish", "/otterscope")).toBe("/otterscopeish");
  });

  it("is a no-op at the root", () => {
    expect(stripBase("/runs/abc", "")).toBe("/runs/abc");
  });
});

describe("route round trip", () => {
  it("returns the route it navigated to, under a prefix", () => {
    for (const route of ["/", "/compare", "/alerts", "/runs/1a2b3c", "/s/tok123"]) {
      expect(stripBase(withBase(route, "/otterscope"), "/otterscope")).toBe(route);
    }
  });
});
