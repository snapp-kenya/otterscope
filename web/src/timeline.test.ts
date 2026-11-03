import { describe, expect, it } from "vitest";
import { barPct } from "./timeline";

describe("barPct", () => {
  it("places an ordinary span proportionally", () => {
    expect(barPct(250, 500, 1000)).toEqual({ left: 25, width: 50 });
  });

  // Child spans carry the clock of the process that emitted them; skew can put
  // one before its parent, and the bar must not paint left of the track.
  it("clamps a negative offset to the start of the track", () => {
    expect(barPct(-400, 200, 1000)).toEqual({ left: 0, width: 20 });
  });

  it("gives way at the right edge instead of overshooting it", () => {
    const { left, width } = barPct(900, 800, 1000);
    expect(left).toBe(90);
    expect(width).toBeCloseTo(10);
    expect(left + width).toBeLessThanOrEqual(100);
  });

  it("keeps both edges inside the track for any input", () => {
    const cases: [number, number, number][] = [
      [-1e9, 1, 1000],
      [0, 1e9, 1000],
      [1e9, 1e9, 1000],
      [1e9, 0, 1000],
      [-50, -50, 1000],
      [0, 0, 0],
    ];
    for (const [offset, duration, total] of cases) {
      const { left, width } = barPct(offset, duration, total);
      const label = `offset=${offset} duration=${duration} total=${total}`;
      expect(left, label).toBeGreaterThanOrEqual(0);
      expect(left, label).toBeLessThanOrEqual(100);
      expect(width, label).toBeGreaterThanOrEqual(0);
      expect(left + width, label).toBeLessThanOrEqual(100 + 1e-9);
    }
  });
});
