import { describe, expect, it } from "bun:test";
import {
  fmtCost,
  fmtDuration,
  fmtPct,
  fmtStart,
  fmtTokens,
} from "@/lib/format";

describe("fmtDuration", () => {
  it("uses ms below a second", () => {
    expect(fmtDuration(0)).toBe("0ms");
    expect(fmtDuration(999)).toBe("999ms");
  });

  it("uses seconds below a minute", () => {
    expect(fmtDuration(1000)).toBe("1.0s");
    expect(fmtDuration(59_940)).toBe("59.9s");
  });

  it("uses minutes and seconds above a minute", () => {
    expect(fmtDuration(60_000)).toBe("1m 0s");
    expect(fmtDuration(3_661_000)).toBe("61m 1s");
  });
});

describe("fmtTokens", () => {
  it("passes small counts through unchanged", () => {
    expect(fmtTokens(0)).toBe("0");
    expect(fmtTokens(999)).toBe("999");
  });

  it("abbreviates thousands and millions", () => {
    expect(fmtTokens(1000)).toBe("1.0k");
    expect(fmtTokens(12_500)).toBe("12.5k");
    expect(fmtTokens(1_000_000)).toBe("1.0M");
    expect(fmtTokens(2_450_000)).toBe("2.5M");
  });
});

describe("fmtCost", () => {
  it("renders a dash when there is no cost, never a zero", () => {
    expect(fmtCost(undefined)).toBe("—");
    expect(fmtCost(null)).toBe("—");
  });

  it("scales precision to the magnitude", () => {
    expect(fmtCost(1.234)).toBe("$1.23");
    expect(fmtCost(0.0456)).toBe("$0.046");
    expect(fmtCost(0.000123)).toBe("$0.00012");
  });

  it("marks a partial total as a lower bound", () => {
    expect(fmtCost(1.5, true)).toBe("≥ $1.50");
    expect(fmtCost(1.5, false)).toBe("$1.50");
  });

  it("keeps a genuinely free call distinct from an unknown one", () => {
    expect(fmtCost(0)).toBe("$0.00000");
  });
});

describe("fmtStart", () => {
  it("omits the date for today and includes it otherwise", () => {
    const now = new Date("2026-08-21T12:00:00Z");
    const today = fmtStart("2026-08-21T09:30:00Z", now);
    const earlier = fmtStart("2026-08-14T09:30:00Z", now);

    expect(today.length).toBeGreaterThan(0);
    expect(earlier.length).toBeGreaterThan(today.length);
    expect(earlier.endsWith(today)).toBe(true);
  });
});

describe("fmtPct", () => {
  it("renders a dash when the denominator is zero", () => {
    expect(fmtPct(0, 0)).toBe("—");
  });

  it("renders one decimal place", () => {
    expect(fmtPct(1, 3)).toBe("33.3%");
    expect(fmtPct(2, 2)).toBe("100.0%");
  });
});
