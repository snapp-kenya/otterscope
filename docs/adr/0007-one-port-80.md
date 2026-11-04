# ADR-0007: One HTTP listener on port 80

Date: 2026-11-04 · Status: accepted · Supersedes: the listener/port decision in ADR-0002

## Context

Otterscope is deployed behind Istio, which terminates TLS and always exposes
port 80 to the service. The Go build and the first Rust port exposed two
configurable listeners: the UI/API on `127.0.0.1:8317` and OTLP ingest on
`127.0.0.1:4318`. That meant two flags (`-listen`, `-otlp`), two environment
variables (`OTTERSCOPE_LISTEN`, `OTTERSCOPE_OTLP`), two config-file keys and two
addresses to get right in every deployment. It also made the common failure
mode — an agent exporting to the UI port, or a deployment exposing only one of
the two — a configuration problem the application could not help with.

The platform, not the application, owns exposure: an ingress, a host port
mapping (`-p 8080:80`), a NodePort or a load balancer. TLS termination is the
ingress's job too.

## Decision

- **One listener, always port 80.** The process binds `0.0.0.0:80` and serves
  the UI, JSON API, `/mcp`, `/healthz`, share links and OTLP ingest on it. OTLP
  is `POST /v1/traces` on that listener.
- **No port configuration.** There is no flag, `OTTERSCOPE_*` variable, config
  key or conditional default for the port. `-listen`, `-otlp`,
  `OTTERSCOPE_LISTEN`, `OTTERSCOPE_OTLP` and the `listen`/`otlp` file keys are
  removed. A config file that still contains them is rejected loudly.
- **One app, one base-path strip.** The server route table and the ingest router
  are merged into a single axum app. The configured `-base-path` is stripped
  once at the edge; the ingest router is mounted at the root internally, so
  `/otterscope/v1/traces` works without double mounting.
- **Separate auth, not duplicated auth.** OTLP keeps its own ingest-key auth,
  per-key rate limit and batch caps. The UI/API keep read/admin token scopes and
  the browser Origin/Host guard.
- **Exposure is a deployment choice.** Compose, Istio, Kubernetes and `docker
  run` decide the host-side port. The application only guarantees the container
  side: 80. TLS is out of scope.
- **Containers stay non-root.** The image runs as uid 65532 and binds 80 with
  `CAP_NET_BIND_SERVICE`; the container security contexts drop `ALL` and add
  only that capability. The image is never switched to root.

## Consequences

- Local host runs need privilege: root, `CAP_NET_BIND_SERVICE` (or a container).
  There is deliberately no unprivileged "dev port" escape hatch.
- Old deployments must republish to port 80 (`-p 8080:80`, or a Service/Ingress
  targetPort of 80) and drop `-listen`/`-otlp`. Config files carrying the removed
  keys fail to start instead of silently ignoring them.
- Exporters point at the published base URL without `/v1/traces`
  (`OTEL_EXPORTER_OTLP_ENDPOINT=http://<host>`); OTLP/HTTP appends the path. If
  the host-side mapping differs from 80, the port belongs in that base URL.
- Tests never bind 80: they drive the router directly (`tower::ServiceExt`) or
  bind `127.0.0.1:0`.
