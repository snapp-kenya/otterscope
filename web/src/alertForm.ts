// The alert threshold box is text, and the request body is JSON. `Number("10%")`
// is NaN, JSON serialises NaN as null, and the server decodes null as 0 — an
// alert that fires on every evaluation and never stops, because 0 is below
// every "greater than" condition. So the text is parsed here, once, and a value
// that is not a finite number never reaches the request. An empty box is
// rejected for the same reason: `Number("")` is 0.
export type ThresholdResult =
  | { ok: true; value: number }
  | { ok: false; error: string };

export function parseThreshold(raw: string, hint: string): ThresholdResult {
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
}
