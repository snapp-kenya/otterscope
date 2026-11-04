export const barPct = (
  offsetMs: number,
  durationMs: number,
  totalMs: number,
): { left: number; width: number } => {
  const total = Math.max(totalMs, 1);
  const pct = (ms: number) => Math.min(100, Math.max(0, (ms / total) * 100));
  const left = pct(offsetMs);
  const width = Math.min(Math.max(pct(durationMs), 0.5), 100 - left);
  return { left, width: Math.max(width, 0) };
};
