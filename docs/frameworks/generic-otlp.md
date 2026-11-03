# Any framework / hand-rolled agents → Otterscope

Otterscope accepts standard OTLP/HTTP on `:4318` and understands the
OpenTelemetry GenAI semantic conventions (both the current and the
`gen_ai_latest_experimental` attribute dialects) plus OpenInference.

Minimum useful span attributes:

| Attribute | Example | Effect |
|---|---|---|
| `gen_ai.operation.name` | `chat`, `execute_tool`, `invoke_agent` | step classification |
| `gen_ai.request.model` | `claude-sonnet-5` | model identity + cost |
| `gen_ai.provider.name` (or `gen_ai.system`) | `anthropic` | provider |
| `gen_ai.usage.input_tokens` / `output_tokens` | `812` / `142` | tokens + cost |
| `gen_ai.tool.name` | `lookup_order` | tool identity |
| `gen_ai.input.messages` / `gen_ai.output.messages` | JSON | inspector conversations |

Rules of thumb:

- One trace = one run. Give the whole agent execution a root span
  (`invoke_agent <name>`) and parent everything under it.
- Spans Otterscope doesn't recognize are kept as generic steps — you lose
  classification, never data.
- Unknown models get tokens but no cost (add rates via `serve -pricing`).
- Raw payloads are retained, so runs re-normalize with future Otterscope
  improvements — instrument now, benefit later. After upgrading, run
  `otterscope renormalize` to backfill existing data (safe to repeat;
  for large stores prefer running it while `serve` is stopped).

## Limits

- One request may carry at most 10,000 spans and touch at most 1,000 distinct
  trace ids (`-ingest-max-spans`, `-ingest-max-trace-ids`). Over either cap
  the whole batch is refused with a 400 before normalization, so retrying
  with a smaller batch costs nothing.
- One ingest key may send 50 batches a second with a burst of 100 by default
  (`-ingest-rate`, `-ingest-burst`; 0 turns the limit off).
- With `-read-auth` on, every request needs a project ingest key — the
  keyless `default` project is closed.
- A span id that already exists under a different service is rejected and
  counted in `partialSuccess.rejectedSpans`. It cannot overwrite another
  service's step; check which service owns the id before retrying.
