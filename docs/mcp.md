# Querying your traces from an agent (MCP)

Otterscope exposes its data as an [MCP](https://modelcontextprotocol.io)
server, so an agent — Claude Code, the MCP Inspector, anything that speaks
MCP — can ask about its own runs: *"why did my last run fail?"*, *"what did
that tool loop cost?"*, *"is my error rate up this hour?"*

The endpoint is **`POST /mcp`** on the same port as the UI (`:8317`), using
the Streamable HTTP transport. It's read-only. Under `-base-path /otterscope`
the endpoint is `/otterscope/mcp`, like every other route.

With `-read-auth` on, `/mcp` requires the same Bearer token as the API; read
scope is enough. Pass it as an `Authorization: Bearer <token>` header from
your MCP client (Claude Code takes `--header`).

## Tools

| Tool | Does |
|---|---|
| `list_runs` | Recent runs, newest first; filter by project, status, model, service, limit. |
| `get_run` | One run by id with its full step tree, including LLM messages and tool arguments/results. |
| `get_stats` | Aggregates over a filtered window: count, error rate, p50/p95 latency, cost, tokens, assertion pass rates. |
| `list_assertions` | The eval assertions configured for a project. |

## Connect Claude Code

```sh
claude mcp add otterscope --transport http http://localhost:8317/mcp
```

Then ask it things like *"use otterscope to show my last 5 runs and tell me
which was slowest"* or *"get the steps of run <id> and explain where it went
wrong."*

## Connect any MCP client

Point it at `http://localhost:8317/mcp` with the Streamable HTTP transport
(add the base path if the server is mounted under one). The server implements
protocol revision `2026-07-28` only, the stateless revision: there is no
`initialize` handshake and no `Mcp-Session-Id`. Clients send
`MCP-Protocol-Version`, `Mcp-Method` and (for `tools/call`) `Mcp-Name` headers,
with the protocol version and client capabilities mirrored in each request's
`_meta`; older protocol revisions are rejected with
`UnsupportedProtocolVersionError`.

## A note on exposure

With `-read-auth` off, the MCP endpoint has no auth — the same posture as the
rest of the API. Turn it on before exposing the instance; `/mcp` then needs a
read token and OTLP ingest needs a project key. The whole API, not just `/mcp`,
checks `Origin` and `Host` on every request, so a malicious page cannot use
your browser to read or write a local instance. Keep it on loopback (the
default) or behind your own firewall/tunnel. It's read access to your trace
data; treat it like the UI.
