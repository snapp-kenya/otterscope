import { describe, expect, it } from "bun:test";
import { BASE_PATH, normalizeBase, withBase } from "@/lib/base";

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

  it("derives BASE_PATH from the build-time base URL", () => {
    expect(BASE_PATH).toBe(normalizeBase(import.meta.env.BASE_URL));
  });
});

describe("withBase", () => {
  it("leaves app paths alone at the root", () => {
    expect(withBase("/api/runs", "")).toBe("/api/runs");
    expect(withBase("/", "")).toBe("/");
  });

  it("prefixes API calls and links under a base path", () => {
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
    expect(withBase("/otterscope/api/runs", "/otterscope")).toBe(
      "/otterscope/api/runs",
    );
    expect(withBase("/otterscope/api/runs?limit=1", "/otterscope")).toBe(
      "/otterscope/api/runs?limit=1",
    );
    expect(withBase("/otterscopeish/api", "/otterscope")).toBe(
      "/otterscope/otterscopeish/api",
    );
  });
});
