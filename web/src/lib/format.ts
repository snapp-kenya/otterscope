export const fmtDuration = (ms: number): string => {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
};

export const fmtTokens = (n: number): string => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
};

export const fmtStart = (iso: string, now: Date = new Date()): string => {
  const at = new Date(iso);
  const sameDay = at.toDateString() === now.toDateString();
  return sameDay
    ? at.toLocaleTimeString()
    : `${at.toLocaleDateString()} ${at.toLocaleTimeString()}`;
};

export const fmtCost = (usd?: number | null, partial?: boolean): string => {
  if (usd === undefined || usd === null) return "\u2014";
  const lowerBound = partial ? "\u2265 " : "";
  if (usd >= 1) return `${lowerBound}$${usd.toFixed(2)}`;
  if (usd >= 0.01) return `${lowerBound}$${usd.toFixed(3)}`;
  return `${lowerBound}$${usd.toFixed(5)}`;
};

export const fmtPct = (n: number, d: number): string =>
  d === 0 ? "\u2014" : `${((100 * n) / d).toFixed(1)}%`;
