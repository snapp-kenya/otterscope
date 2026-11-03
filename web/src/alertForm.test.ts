import { describe, expect, it } from "vitest";
import { parseThreshold } from "./alertForm";

const FRACTION = "fraction 0–1 (e.g. 0.1 = 10%)";

describe("parseThreshold", () => {
  it("accepts the number the form starts with", () => {
    expect(parseThreshold("0.1", FRACTION)).toEqual({ ok: true, value: 0.1 });
  });

  it("accepts the other units alerts use", () => {
    expect(parseThreshold("250", "milliseconds")).toEqual({ ok: true, value: 250 });
    expect(parseThreshold("12.5", "USD total in window")).toEqual({
      ok: true,
      value: 12.5,
    });
  });

  it("trims before parsing", () => {
    expect(parseThreshold("  0.05  ", FRACTION)).toEqual({ ok: true, value: 0.05 });
  });

  // "10%" is the natural thing to type and the worst thing to accept:
  // Number("10%") is NaN, JSON writes NaN as null, and the server stores null
  // as 0 — an alert that fires on every evaluation.
  it("rejects a percentage with an explanation", () => {
    const got = parseThreshold("10%", FRACTION);
    expect(got.ok).toBe(false);
    if (!got.ok) {
      expect(got.error).toMatch(/0\.1/);
      expect(got.error).toContain(FRACTION);
    }
  });

  // Number("") is 0, which is finite, so an empty box would save a threshold
  // of zero rather than failing.
  it("rejects an empty box rather than treating it as zero", () => {
    const got = parseThreshold("   ", FRACTION);
    expect(got.ok).toBe(false);
  });

  it("rejects non-numeric text", () => {
    for (const text of ["abc", "0.1x", "--", "$5"]) {
      expect(parseThreshold(text, FRACTION).ok, text).toBe(false);
    }
  });

  // JSON.stringify(Infinity) is null too, so finiteness is the real test.
  it("rejects Infinity and NaN", () => {
    expect(parseThreshold("Infinity", FRACTION).ok).toBe(false);
    expect(parseThreshold("NaN", FRACTION).ok).toBe(false);
  });

  it("never returns a non-finite value", () => {
    for (const text of ["", "10%", "abc", "Infinity", "1e999"]) {
      const got = parseThreshold(text, FRACTION);
      if (got.ok) expect(Number.isFinite(got.value)).toBe(true);
    }
  });
});
