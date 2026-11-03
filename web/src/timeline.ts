// Geometry for a timeline bar, as percentages of the run's own duration.
// Offsets come from the clock of whichever process emitted the span, so skew
// can put a child before its parent (a negative offset) or past the run's end.
// Left unclamped, the bar renders outside the track; clamp both edges and let
// the width give way at the right edge instead of overshooting it.
export function barPct(
  offsetMs: number,
  durationMs: number,
  totalMs: number,
): { left: number; width: number } {
  const total = Math.max(totalMs, 1);
  const pct = (ms: number) => Math.min(100, Math.max(0, (ms / total) * 100));
  const left = pct(offsetMs);
  // Keep sub-pixel steps visible, but never past the end of the track.
  const width = Math.min(Math.max(pct(durationMs), 0.5), 100 - left);
  return { left, width: Math.max(width, 0) };
}
