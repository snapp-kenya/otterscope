---
description: "Use when writing, reviewing, or testing Rust backends, Rust source, Cargo manifests and lockfiles, cargo configuration, or Rust toolchain files. Enforces safe Rust, bounded async work, contract preservation, and container-only verification."
applyTo: "**/*.rs,**/Cargo.toml,**/Cargo.lock,**/rust-toolchain,**/rust-toolchain.toml,**/.cargo/config,**/.cargo/config.toml"
changes:
  - date: 2026-10-12
    ticket: [HITL]
    description: Added the Failure behaviour section forbidding services from detecting or compensating for missing dependencies, added the Platform security section, made the tenant a platform-bound value the service never handles, and moved acting-identity validation to the platform.
    agent: Default Agent
  - date: 2026-10-01
    ticket: [HITL]
    description: init
    agent: sys.agent-writer
---

# Rust Coding Standards

## Scope and authority

1. First-party APIs, projectors, consumers, agent runtimes, MCP services,
   operators, event publishers, adapters, and sidecars MUST be Rust.
2. A non-Rust first-party backend needs a compelling documented reason and
   explicit human approval. A bounded niche-library proof of concept needs a
   named owner, expiry and replacement decision; it is not production approval.
3. Upstream third-party products retain their languages. Their first-party
   adapters remain Rust. JavaScript and TypeScript are UI or Bun automation;
   Bash is automation, not a backend implementation loophole.
4. Read the relevant component, security, API and event specifications before
   implementation. Keep changes within caller-assigned ownership. Missing
   source or an unspecified contract is a hand-back, not permission to invent
   a service or sidecar.

## Toolchain, style and ownership

1. Use the current stable Rust release at adoption or upgrade, edition
   `2024`, and no nightly features. Edition 2024 first shipped in Rust 1.85;
   that is an edition minimum, not the current stable version. Resolve and pin
   an exact stable version in `rust-toolchain.toml` and the builder image,
   including its digest; do not leave CI on a floating `stable` or `latest`.
2. Declare `edition = "2024"` and an honest `rust-version` in Cargo manifests;
   inherit workspace policy consistently. Update compiler pins deliberately
   with compatibility and security evidence, never silently during a build.
3. Prefer simple modules, explicit domain types and composition. Use standard
   Rust naming, rustfmt, documented public interfaces and small functions.
   Comments explain invariants and cite the governing specification section;
   they do not restate each line or disguise unfinished work.
4. Borrow rather than clone without a reason. Make ownership and lifetimes
   explicit at boundaries. Avoid unnecessary `Arc`, shared mutable state and
   abstraction. Keep lock scopes short and never hold a blocking mutex guard
   across `.await`. Document lock order where several locks are required.
5. Keep dependencies minimal and use standard-library functionality when it
   suffices. Do not compress logic at the expense of readable error handling,
   checked arithmetic or clear ownership.

## Safe Rust and dependency governance

1. Safe Rust is the default. New first-party crates use
   `#![forbid(unsafe_code)]` unless an explicitly reviewed FFI or other necessary
   unsafe boundary is approved. Any exception must be tiny, isolated and
   documented with `SAFETY` invariants, ownership, lifetimes and tests. Review
   transitive native code too; safe application code does not make FFI safe.
2. Do not implement, duplicate or bypass platform security. Istio supplies
   transport security, SPIFFE/SPIRE supplies workload identity and OpenBao
   supplies secrets. Follow [Platform security](#platform-security) below.
3. Justify each dependency by capability, alternatives, recreation cost,
   maintenance, licence, advisories, transitive cost and enabled features.
   Mature security and protocol libraries are not replaced for dependency
   minimalism. Libraries such as Tokio, tracing or a Kafka client are
   candidates, not an unconditional approved stack.
4. Commit `Cargo.lock` for service workspaces. Pin Git dependencies to full
   revisions. Review lockfile changes, build scripts, proc macros, native
   libraries and feature expansion; disable unused default features when
   compatible. Do not use unreviewed sources or fetch code at runtime.
5. Run RustSec audit and the existing licence, vulnerability and supply-chain
   checks. Record narrowly scoped advisory exceptions with approval, owner and
   expiry. Do not suppress a finding globally. Apply the existing vendor-risk
   policy without adding a proprietary-vendor ban or exception.

## Platform security

1. First-party services do not implement security primitives. Cryptography,
   hashing, password storage, key derivation, random generation, constant-time
   comparison, MAC and signature construction, token parsing and verification,
   certificate validation and envelope encryption are all supplied by the
   platform. Reimplementing any of them is forbidden, including a "temporary"
   or "minimal" local variant.
2. Every workload serves plain HTTP on its pod port and MUST NOT terminate TLS.
   North-south TLS terminates at the Istio ingress gateway, and east-west
   traffic is mTLS between Istio sidecars. Do not add a TLS listener, an HTTPS
   endpoint, a certificate, a private key, a trust bundle or certificate
   pinning to a service. Do not add `rustls`, `native-tls`, `openssl`,
   `jsonwebtoken`, `ring` or an equivalent to a first-party service unless the
   ticket records an approved platform exception.
3. Identity comes from SPIFFE/SPIRE over the Workload API socket. The SVID is
   the only accepted workload credential for east-west mTLS, secrets-manager
   authentication and bus ACL enforcement. Do not read an SVID, certificate or
   private key from disk, and do not mint, cache beyond its lease, or forward
   another workload's identity.
4. Authorisation is a default-deny Istio `AuthorizationPolicy` written against
   SPIFFE ID prefixes. A service MUST NOT make its own authentication or
   authorisation decision. Where a callee must authorise on acting identity, the
   platform validates on its behalf: Istio `RequestAuthentication` checks the
   token against the IdP JWKS at the boundary, and the `ext_authz` service
   performs the fine-grained check. The service consumes only the actor claims
   the platform has already validated, and never parses, verifies or
   second-guesses a token itself (rule 1).
5. Secrets come from the secrets manager as short-lived, single-use leases
   materialised on `tmpfs`. Never read a secret from an environment variable, a
   committed file, a container image layer or a build argument. A second fetch
   of an active lease is a breach signal and MUST NOT be attempted.
6. Never disable or weaken a security control to make a path, a test or a local
   run pass. `danger_accept_invalid_certs`, insecure skip-verify flags,
   plaintext fallbacks and an `insecure` feature that can reach a deployed
   environment are forbidden.
7. Treat a request that reaches the application as already authenticated and
   authorised by the platform. Do not re-check identity, scope or tenant, and do
   not widen the access the injected connection already grants.

## Failure behaviour

1. A service MUST NOT detect or react to a missing platform or downstream
   dependency. When an operation it needs fails, or a dependency is absent, it
   fails the request and exits. Restart, redelivery, retry and dead-lettering
   belong to the platform, not to the service.
2. Do not implement compensation for platform failure: no outbox, no
   store-and-forward queue, no drain-on-recovery worker, no degraded mode, no
   fallback path, no circuit breaker, no reconnect loop, and no bespoke recovery
   state. Reporting success for work that did not complete, including returning
   `accepted` for an event that was never published, is forbidden.
3. Do not implement delivery semantics the platform already provides. The bus
   sidecar owns retry, offset commit and dead-lettering; the mesh owns timeouts
   and connection handling; the credential owns isolation. Reimplementing any of
   them is a defect.
4. A failure surfaces by failing the request and exiting non-zero, observed
   through the platform's error and log telemetry. It MUST NOT surface as
   invented queue depth, retry counters or recovery artefacts.

## Errors, input and arithmetic

1. Use `Result` for fallible operations and `Option` for absence; domain
   errors must distinguish invalid input, conflict, denial, dependency failure
   and cancellation. Add context without leaking sensitive values. Map errors
   to the existing HTTP, MCP and event contracts, not a new error envelope.
2. Propagate errors with `?`; do not silently convert outages to success or
   empty results. Avoid `unwrap`, `expect`, panic and unchecked indexing on
   production paths. Tests may assert with these; an unavoidable invariant
   exception requires a documented proof and review. Panics are not request
   handling, retry or control flow.
3. Validate data at every trust boundary, including events, configuration,
   identifiers and database records. Enforce tenant scope, size, depth, count,
   range and encoding limits before expensive allocation or processing. Use
   parameterised queries; confine paths and outbound addresses to their
   authorised roots and destinations.
4. Use `checked_add`, `checked_sub`, `checked_mul` and fallible conversions
   such as `TryFrom` for input-derived lengths, offsets, counts and money.
   Check division and overflow explicitly; do not hide invalid values through
   wrapping, saturating, lossy casts or release-mode overflow behaviour.
   Represent money and quantities with contract-defined units and precision.

## Secrets, privacy and isolation

1. Use platform workload identity and approved secret delivery. Never embed
   credentials in source, image layers, build arguments, URLs, errors or logs.
   Keep secret lifetimes and copies short; use reviewed secret wrappers and
   zeroisation when the threat model requires them, without claiming that all
   copies or allocator memory have been erased.
2. Never log tokens, passwords, cookies, raw bodies, keys or PII. Explicitly
   skip sensitive arguments in instrumented spans and redact structured
   fields before export. Do not derive `Debug` for secret-bearing types unless
   output is safely redacted. Use approved pseudonymous correlation fields.
3. Never handle a tenant identifier in code. The tenant is bound by identity and
   connection, not by any value the service passes around: the platform derives
   it from the SPIFFE identity and the validated JWT claim and hands the service
   a connection, endpoint or namespace that is already scoped to that tenant. Do
   not read, derive, construct, compare, log or forward a tenant identifier, and
   never accept one from a caller, a request, configuration or an environment
   variable. Where a shared substrate needs a tenant-qualified name, the platform
   layer applies it.

## Async, bounds and cancellation

1. Use an approved async runtime only where needed. Bound concurrency with
   permits, queues and pools; cap body size, queue depth, batches, connection
   count, retained data and task fan-out. No detached request tasks or unbounded
   channels. Define backpressure and overload behaviour in the contract.
2. Every external operation has a deadline and a cancellation path. Preserve
   the approved retry semantics, with bounded attempts and jittered backoff
   where retries are permitted. Never retry non-idempotent effects blindly.
   Tokio timeout drops a future; it does not undo external effects or terminate
   already-running blocking work. Audit cancellation safety of `select!`
   branches, partial writes, commits and third-party client calls.
3. Track spawned work, handle join failures and propagate cancellation. Offload
   blocking or CPU-heavy work to a bounded pool; do not block runtime threads.
   Already-running `spawn_blocking` work is not forcibly cancelled by dropping
   its handle; provide cooperative bounds and account for shutdown time.
4. On SIGTERM stop admission, cancel or drain tracked work within a fixed
   budget, flush permitted telemetry, close clients and exit predictably.
   Readiness reflects draining and dependency health. Test shutdown under load,
   dependency stalls and cancellation between effects and acknowledgements.

## Observability and CQRS contracts

1. Use structured tracing and metrics integrated with the platform's
   OpenTelemetry pipeline. Emit single-record structured lifecycle, readiness,
   dependency loss/recovery, draining and error events.
2. Propagate approved trace and correlation context across HTTP and events.
   Measure latency, errors, throughput, queue depth, retries and saturation.
   Keep label cardinality bounded: no raw tenant, user, request or event IDs
   as metric labels. Avoid sensitive payloads and unbounded span attributes.
3. Preserve CQRS: command paths validate and publish CloudEvent-wrapped domain
   events; projectors build read models; query paths do not perform hidden
   writes. Use the existing schemas, topics, identity and idempotency rules.
4. For bus adapters and sidecars, preserve the bus contract: payloads,
   headers, ordering, groups, retries, handler HTTP behaviour, acknowledgement
   and offset-commit points. Document and test actual failure semantics. Do not
   invent a DLQ, transaction or exactly-once guarantee.

## Testing and failure-path evidence

1. Write deterministic unit tests for domain invariants, validation, errors
   and boundary arithmetic. Use property tests or fuzzing for parsers and
   input-dependent logic when justified; run their tooling in containers.
2. Use integration and contract tests against containerised dependencies and
   existing HTTP/event fixtures. Cover malformed and oversized input,
   authentication and cross-tenant denial, duplicate events, ordering,
   retry/commit failures, dependency outages and recovery, overload,
   cancellation and graceful shutdown. Match failure expectations to the
   existing contract, not an invented guarantee.
3. Exercise the deployed feature set and each supported architecture. Test
   documented feature combinations; `--all-features` is not sufficient where
   features are incompatible or defaults are significant. Include doctests
   explicitly because `--all-targets` does not run them.

## Reproducible container-only verification

1. Install no host Rust, Cargo, rustfmt, clippy, audit tooling or native Kafka
   libraries. All source builds, formatting, linting, tests, audits and
   packaging run in containers. Host Git and read-only discovery are allowed.
   Read the [Dockerfile coding standard](dockerfile-coding.instructions.md)
   before changing images, and follow the repository's shell conventions
   before writing automation.
2. Use repository-owned, digest-pinned builder and audit images with the exact
   stable compiler, rustfmt, clippy and pinned cargo-audit already included.
   Do not install tools dynamically during verification. Record source SHA,
   image digest, compiler/tool versions, feature set and target architecture.
3. Fetch locked dependencies in a controlled container preparation step;
   build/test offline with `--frozen` once the cache or vendored sources are
   ready. Use `--locked` when the approved harness requires fetch access. A
   lockfile alone does not pin system libraries or build-script downloads.
   Pin those inputs too. Keep caches isolated by toolchain, target and trust.
4. The following are commands INSIDE the approved verification containers,
   not host commands or claims that a harness already exists:

   | Gate | Container command / required evidence |
   | --- | --- |
   | fmt | `cargo fmt --all -- --check` |
   | clippy | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` |
   | build | `cargo build --workspace --release --locked` with the approved production features and target |
   | test | `cargo test --workspace --all-targets --all-features --locked`, plus `cargo test --workspace --doc --all-features --locked` and deployed/default feature coverage |
   | audit | `cargo audit --file Cargo.lock` with pinned cargo-audit and a fresh, recorded RustSec database revision |

   Replace `--locked` with `--frozen` after preparation. Use an explicitly
   documented feature matrix instead of `--all-features` for incompatible
   features. Audit is not a Cargo build and does not take the build's lock
   flag; advisory freshness and database revision must be recorded separately.
5. Local Docker Compose is a small verification harness outside Kubernetes,
   not a production reference. Support native `linux/arm64` on Apple Silicon
   and the declared production targets, typically `linux/amd64`. Do not force
   AMD64 emulation as the only path. Cross-compilation or emulation alone does
   not prove runtime compatibility: execute tests on each release target and
   record native dependencies, ABI and TLS compatibility. Bound build jobs,
   memory and test parallelism to the local machine's container budget.
6. Runtime images contain only required artefacts and libraries, run with
   least privilege and do not contain compilers or source secrets. Produce
   SBOM, scans, signatures and provenance for release images.
7. Tekton provides separate `fmt`, `clippy`, `build`, `test` and `audit` tasks
   against the same SHA, lockfile, builder pins and feature/target matrix.
   Failure of any required gate blocks release-candidate promotion. Retain
   task evidence and sign the multi-architecture manifest only after all
   target gates pass. Reproducibility means recorded, controlled inputs; do
   not claim bit-for-bit reproducibility without comparing outputs.
8. Report missing source, images, Compose descriptors, Tekton tasks, services
   or network access as gaps. Do not claim unrun checks passed or implement
   speculative sidecar source merely to obtain a green check.

## Official references

1. [Rust edition 2024](https://doc.rust-lang.org/edition-guide/rust-2024/index.html).
2. [Cargo build and lockfile options](https://doc.rust-lang.org/cargo/commands/cargo-build.html).
3. [Clippy usage](https://doc.rust-lang.org/clippy/usage.html).
4. [RustSec audit tooling](https://rustsec.org/).
5. [Tokio graceful shutdown](https://tokio.rs/tokio/topics/shutdown) and
   [select cancellation safety](https://docs.rs/tokio/latest/tokio/macro.select.html).