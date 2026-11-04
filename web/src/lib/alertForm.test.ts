import { describe, expect, it } from "bun:test";
import { parseThreshold } from "@/lib/alertForm";

const FRACTION = "fraction 0–1 (e.g. 0.1 = 10%)";

describe("parseThreshold", () => {
  it("accepts the number the form starts with", () => {
    expect(parseThreshold("0.1", FRACTION)).toEqual({ ok: true, value: 0.1 });
  });

  it("accepts the other units alerts use", () => {
    expect(parseThreshold("250", "milliseconds")).toEqual({
      ok: true,
      value: 250,
    });
    expect(parseThreshold("12.5", "USD total in window")).toEqual({
      ok: true,
      value: 12.5,
    });
  });

  it("trims before parsing", () => {
    expect(parseThreshold("  0.05  ", FRACTION)).toEqual({
      ok: true,
      value: 0.05,
    });
  });

  it("rejects a percentage with an explanation", () => {
    const result = parseThreshold("10%", FRACTION);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error).toMatch(/0\.1/);
      expect(result.error).toContain(FRACTION);
    }
  });

  it("rejects an empty box rather than treating it as zero", () => {
    expect(parseThreshold("   ", FRACTION).ok).toBe(false);
  });

  it("rejects non-numeric text", () => {
    for (const text of ["abc", "0.1x", "--", "$5"]) {
      expect(parseThreshold(text, FRACTION).ok, text).toBe(false);
    }
  });

  it("rejects Infinity and NaN", () => {
    expect(parseThreshold("Infinity", FRACTION).ok).toBe(false);
    expect(parseThreshold("NaN", FRACTION).ok).toBe(false);
  });

  it("never returns a non-finite value", () => {
    for (const text of ["", "10%", "abc", "Infinity", "1e999"]) {
      const result = parseThreshold(text, FRACTION);
      if (result.ok) expect(Number.isFinite(result.value)).toBe(true);
    }
  });
});
