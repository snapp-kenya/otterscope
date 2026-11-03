# Connecting your agent framework

Otterscope ingests standard OpenTelemetry traces over OTLP/HTTP. Anything
that exports OTel spans works; the guides below cover the fast path for
common frameworks. In every case the core is the same two variables:

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
# required when the server runs -read-auth; otherwise only if you created a
# project key:
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <ingest-key>"
```

Under `-base-path /otterscope`, point the exporter at
`http://localhost:4318/otterscope` — OTLP/HTTP appends `/v1/traces` itself.
One batch may carry at most 10,000 spans and 1,000 distinct traces; a larger
one is refused whole. See [generic-otlp.md](generic-otlp.md#limits).

| Framework | Guide | Dialect Otterscope normalizes |
|---|---|---|
| Pydantic AI | [pydantic-ai.md](pydantic-ai.md) | OTel GenAI (native) |
| OpenAI Agents SDK | [openai-agents.md](openai-agents.md) | OpenInference |
| Vercel AI SDK | [vercel-ai-sdk.md](vercel-ai-sdk.md) | ai.* + gen_ai hybrid |
| LangGraph / LangChain | [langgraph.md](langgraph.md) | OTel GenAI via instrumentor |
| Claude Agent SDK / Claude Code | [claude-agent-sdk.md](claude-agent-sdk.md) | `claude_code.*` (its own dialect) |
| Anything else | [generic-otlp.md](generic-otlp.md) | OTel GenAI conventions |

Message content (prompts/completions) is opt-in in most instrumentations —
each guide shows the flag. Without it you still get runs, steps, latency,
tokens, and cost; with it the run inspector shows full conversations. The
exception is the Claude Agent SDK, which sends conversation content over the
logs protocol rather than on spans, so its runs never carry messages — see
that guide for what it does carry.
