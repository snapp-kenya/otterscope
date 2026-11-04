import { describe, expect, it } from "bun:test";
import {
  emptyFilters,
  filtersFromSearch,
  filtersToQuery,
  hasActiveFilters,
  PAGE_SIZE,
  parseRunsSearch,
  rangeSince,
  searchFromFilters,
  withFilterDefaults,
  type FilterState,
} from "@/lib/filters";

const empty: FilterState = { ...emptyFilters };
const NOW = Date.parse("2026-08-21T12:00:00.000Z");

describe("filtersToQuery", () => {
  it("always caps the page size", () => {
    expect(new URLSearchParams(filtersToQuery(empty)).get("limit")).toBe("100");
  });

  it("omits empty fields rather than sending blanks", () => {
    const params = new URLSearchParams(filtersToQuery(empty));
    expect([...params.keys()]).toEqual(["limit"]);
  });

  it("sends every filter the user can set", () => {
    const params = new URLSearchParams(
      filtersToQuery(
        {
          q: "term",
          project: "alpha",
          status: "error",
          model: "claude",
          service: "svc",
          prompt: "p1",
          range: "24h",
        },
        0,
        PAGE_SIZE,
        NOW,
      ),
    );
    for (const key of [
      "q",
      "project",
      "status",
      "model",
      "service",
      "prompt",
      "since",
    ]) {
      expect(params.has(key), `${key} missing from the request`).toBe(true);
    }
  });

  it("passes every set field through", () => {
    const params = new URLSearchParams(
      filtersToQuery({
        ...empty,
        project: "alpha",
        status: "error",
        model: "claude",
        service: "svc",
        prompt: "p1",
      }),
    );
    expect(params.get("project")).toBe("alpha");
    expect(params.get("status")).toBe("error");
    expect(params.get("model")).toBe("claude");
    expect(params.get("service")).toBe("svc");
    expect(params.get("prompt")).toBe("p1");
  });

  it("turns a relative range into an RFC 3339 since bound", () => {
    const since = (range: string) =>
      new URLSearchParams(
        filtersToQuery({ ...empty, range }, 0, PAGE_SIZE, NOW),
      ).get("since");

    expect(since("1h")).toBe("2026-08-21T11:00:00.000Z");
    expect(since("24h")).toBe("2026-08-20T12:00:00.000Z");
    expect(since("7d")).toBe("2026-08-14T12:00:00.000Z");
  });

  it("sends no bound for an unrecognised range", () => {
    const params = new URLSearchParams(
      filtersToQuery({ ...empty, range: "not-a-range" }, 0, PAGE_SIZE, NOW),
    );
    expect(params.has("since")).toBe(false);
  });

  it("does not mangle a term with spaces or punctuation", () => {
    const params = new URLSearchParams(
      filtersToQuery({ ...empty, q: 'error: "timed out" & retry' }),
    );
    expect(params.get("q")).toBe('error: "timed out" & retry');
  });
});

describe("rangeSince", () => {
  it("returns null for an empty or unknown range", () => {
    expect(rangeSince("", NOW)).toBeNull();
    expect(rangeSince("all", NOW)).toBeNull();
  });
});

describe("filtersToQuery paging", () => {
  it("asks for no offset on the first page", () => {
    expect(new URLSearchParams(filtersToQuery(empty, 0)).has("offset")).toBe(
      false,
    );
  });

  it("offsets by whole pages", () => {
    const offset = (page: number) =>
      new URLSearchParams(filtersToQuery(empty, page)).get("offset");

    expect(offset(1)).toBe(String(PAGE_SIZE));
    expect(offset(3)).toBe(String(PAGE_SIZE * 3));
  });

  it("keeps the page size off the offset when asking for the lookahead row", () => {
    const params = new URLSearchParams(
      filtersToQuery(empty, 2, PAGE_SIZE + 1),
    );
    expect(params.get("limit")).toBe(String(PAGE_SIZE + 1));
    expect(params.get("offset")).toBe(String(PAGE_SIZE * 2));
  });

  it("still sends the filters alongside the offset", () => {
    const params = new URLSearchParams(
      filtersToQuery({ ...empty, q: "invoice" }, 1),
    );
    expect(params.get("q")).toBe("invoice");
    expect(params.get("offset")).toBe(String(PAGE_SIZE));
  });
});

describe("withFilterDefaults", () => {
  it("fills the keys a view saved before they existed never had", () => {
    const applied = withFilterDefaults({ model: "claude", status: "error" });
    expect(applied).toEqual({ ...empty, model: "claude", status: "error" });
    for (const value of Object.values(applied)) {
      expect(typeof value).toBe("string");
    }
  });

  it("drops nulls and non-strings instead of passing them through", () => {
    const applied = withFilterDefaults({ q: null, prompt: 7 });
    expect(applied.q).toBe("");
    expect(applied.prompt).toBe("");
  });

  it("returns the full default set when a view has no params at all", () => {
    expect(withFilterDefaults(null)).toEqual(emptyFilters);
    expect(withFilterDefaults(undefined)).toEqual(emptyFilters);
    expect(withFilterDefaults("nonsense")).toEqual(emptyFilters);
  });

  it("keeps a range the view does carry", () => {
    expect(withFilterDefaults({ range: "24h" }).range).toBe("24h");
  });
});

describe("URL search mapping", () => {
  it("omits empty filters so a shared link stays clean", () => {
    expect(searchFromFilters(empty)).toEqual({});
  });

  it("round-trips every filter, including special characters", () => {
    const filters: FilterState = {
      q: "error & timeout",
      project: "alpha/beta",
      status: "error",
      model: "claude-sonnet-5",
      service: "svc name",
      prompt: "p 1",
      range: "7d",
    };
    const search = searchFromFilters(filters, 3);
    expect(parseRunsSearch(search as Record<string, unknown>)).toEqual({
      ...search,
      page: 3,
    });
    expect(filtersFromSearch(parseRunsSearch(search as Record<string, unknown>))).toEqual(
      filters,
    );
  });

  it("parses only string filters and a positive integer page", () => {
    expect(
      parseRunsSearch({ q: 5, project: "alpha", page: "2", status: null }),
    ).toEqual({ project: "alpha", page: 2 });
    expect(parseRunsSearch({ page: "-1" }).page).toBeUndefined();
    expect(parseRunsSearch({ page: "1.5" }).page).toBeUndefined();
  });

  it("ignores query params it does not own", () => {
    const search = parseRunsSearch({ project: "alpha", unrelated: "x" });
    expect(search.project).toBe("alpha");
    expect("unrelated" in search).toBe(false);
  });
});

describe("hasActiveFilters", () => {
  it("is false only for the full default set", () => {
    expect(hasActiveFilters(empty)).toBe(false);
    for (const key of Object.keys(emptyFilters) as (keyof FilterState)[]) {
      expect(hasActiveFilters({ ...empty, [key]: "set" })).toBe(true);
    }
  });
});
