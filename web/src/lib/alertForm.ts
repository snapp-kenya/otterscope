export type ThresholdResult =
  | { ok: true; value: number }
  | { ok: false; error: string };

export const parseThreshold = (
  raw: string,
  hint: string,
): ThresholdResult => {
  const text = raw.trim();
  if (text === "") {
    return { ok: false, error: `Enter a threshold — ${hint}.` };
  }
  if (text.endsWith("%")) {
    return {
      ok: false,
      error: `Percentages are not accepted: 10% is 0.1 — ${hint}.`,
    };
  }
  const value = Number(text);
  if (!Number.isFinite(value)) {
    return { ok: false, error: `"${text}" is not a number — ${hint}.` };
  }
  return { ok: true, value };
};
