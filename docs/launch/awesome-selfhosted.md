# awesome-selfhosted submission

> **Read this before submitting.** Their guidelines state: *"Machine/LLM-generated
> contributions, that do not respect project guidelines are not allowed and will
> result in a ban."* Everything below is a starting point that a human must read,
> verify against the live guidelines, and submit. Do not paste it unreviewed.

## Where it goes

**Not** `awesome-selfhosted/awesome-selfhosted`. That repo's `README.md` is
generated. Submissions are a **YAML file** in the data repo:

- Repo: <https://github.com/awesome-selfhosted/awesome-selfhosted-data>
- File: `software/otterscope.yml`
- Guidelines: <https://github.com/awesome-selfhosted/awesome-selfhosted-data/blob/master/CONTRIBUTING.md>
- PR template (extra rules): `PULL_REQUEST_TEMPLATE.md` in that repo

Category: **Generative Artificial Intelligence (GenAI)** — the tag Agenta,
Khoj, LibreChat and the other LLM tooling sits under. In single-page mode a
project shows only under the *first* tag listed, so keep GenAI first.

## Status: blocked, do not submit

Their clock is **four months from the first tagged release**, not from repo
creation, and their reviewers check for a tagged release explicitly: a project
showing none gets closed with a request to cut one.

This repository no longer publishes tagged releases. Releases, archives and
every other distribution channel are withdrawn by operator decision
([ADR-0004](../adr/0004-release-channels.md)); the only artifact is the
container image built from `dockerfile`. So this submission cannot proceed as
drafted, and nothing here should be published until that changes. The draft
entry below is kept only as ready-made metadata; the file is a candidate for
deletion.

The Show HN and r/selfhosted drafts have no such gate.

## Draft entry

```yaml
name: Otterscope
website_url: https://github.com/otterscope/otterscope
source_code_url: https://github.com/otterscope/otterscope
description: Observability and evals for AI agents — run timelines, token and cost tracking, assertions and LLM-as-judge scoring over OpenTelemetry traces. Single Rust binary with embedded SQLite. (alternative to Langfuse, Arize Phoenix)
licenses:
  - Apache-2.0
platforms:
  - Rust
  - Docker
tags:
  - Generative Artificial Intelligence (GenAI)
```

Notes on the fields, from their guidelines:

- Descriptions must not say "open-source", "free" or "self-hosted" — the list
  already implies all three.
- Prefer the short form ("Observability and evals for…", not "Otterscope is a…").
- `(alternative to …)` is their convention for positioning against an existing
  product, which is how the README already frames the project.
- `Rust` is listed because the project ships a single self-contained binary,
  and their guidelines ask for the language in that case.
- Leave out `stargazers_count`, `updated_at`, `current_release` and
  `commit_history` — their tooling fills those in.
