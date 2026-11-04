# Workflow: executing an issue or feature

This is the operating procedure for all work on Otterscope, whether done by a human or an AI agent. Its purpose is to keep velocity high without accumulating tech debt: every change is traceable to a tracked item, verified before merge, and reflected in the docs that describe the system.

> **Tracking note (2026-11-04):** GitHub Issues are disabled on this repository (`gh repo view --json hasIssuesEnabled` returns `false`). Until they are re-enabled, the port epic is tracked by the draft PR `rewrite/rust-port` (#1), and follow-up work belongs in that PR's description or review threads rather than a new issue.

## 1. Before writing code

0. **Check nobody is already on it**: `gh pr list --search "<topic>"` and read the draft epic's checklist before writing code. Duplicated work is the normal failure mode; an outside contributor once opened #112 within an hour of #105 being filed, and it was duplicated and merged over because nobody looked.
1. **Work only from a tracked item.** With issues disabled, the item is a line in the port epic (#1), a linked PR, or a follow-up comment on the epic. If the work is not represented there, add it before writing code: problem statement, acceptance criteria, and which milestone it belongs to. Unplanned work discovered mid-task becomes a *new tracked item*, not scope creep on the current one.
2. **Read the context**: `CLAUDE.md`, the relevant ADRs in `docs/adr/`, and the code you're about to touch. ADR-0006 records the Rust port; ADR-0002 (ingest boundary), ADR-0003 (license) and ADR-0005 (self-host only) still hold. If the work contradicts an ADR, resolve that first (new ADR or a comment on the epic), don't code around it.
3. **State the plan** in the PR description or epic checklist: approach, files touched, how it will be verified. This is the tech-debt checkpoint — if the plan needs a hack, say so explicitly and file the follow-up *before* merging the hack.

## 2. While coding

4. **Branch per change**: `feat/<slug>` or `fix/<slug>` off the active integration branch (`rewrite/rust-port` while the port lands) or `main` once it is the build target. The branch that ships stays green and releasable at all times.
5. **Tests are part of the change, not a follow-up.** New behavior → new test. Bug fix → regression test that fails without the fix. Ingest changes → a captured-payload fixture in `rust/crates/otterscope/testdata/`.
6. **Keep the diff scoped to the tracked item.** Drive-by refactors go in their own PR unless they're trivially small and directly enable the change.
7. **New dependency? Justify it** in the commit message (what it does, why std or the existing tokio/axum/rusqlite stack can't, maintenance status).

## 3. Before merging

8. **Verify end-to-end, not just unit tests**: build the binary, run `otterscope serve`, exercise the changed behavior for real (send an OTLP payload, click through the UI change). Record what you did in the PR description ("Verified: …").
9. **Quality gate**: all six must pass locally before merge. There is no CI —
   these commands *are* the gate, so run every one of them from a clean tree
   and record the result in the PR description.

   ```sh
   cd rust && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
   cd web && bun run lint && bun test && bun run build
   ```

   Never merge on a red gate. A flaky failure is still a failure: re-run the
   command and debug it; there is no job to re-run. `bun run build` matters as
   much as the tests: the binary embeds `web/dist` at compile time.
10. **Update the paper trail in the same PR**: ADR if an architectural decision was made, `docs/ROADMAP.md` if scope/timeline shifted, `CLAUDE.md` if the architecture map changed, README if user-facing behavior changed, `docs/api-rust.md` if the HTTP contract changed, and `CHANGELOG.md` under "Unreleased" for anything an operator or API client would notice.
11. **PR → squash-merge** to the integration branch (or `main` once the port lands), with the tracked item linked in the description.
    - When GitHub Issues are re-enabled and a PR closes several of them, repeat the keyword: `Closes #12, Closes #13`. GitHub honours it only for the first number in a comma-separated list, so `Closes #12, #13` closes #12 and silently leaves #13 open — this stranded nine already-fixed issues after PRs #110 and #114.

## 4. After merging

12. **Close the loop**: confirm the tracked item actually closed or the epic checklist was updated — don't assume it did — and move follow-up items into their own tracked items with milestone labels.
13. **Never leave the shipping branch broken.** If the local gate fails after merge, fixing it preempts all other work.

## Tech-debt policy

- Debt is only acceptable when *recorded*: a `debt`-labeled issue (once issues return) or an epic line, linked from a `// TODO(<tracked item>):` comment at the site. TODO comments without a tracked reference fail review.
- Every 4th milestone-week includes a debt-repayment pass: pick the top tracked debt items and clear them before new features.
- The ingest normalization layer is expected churn (upstream OTel GenAI conventions are unstable) — quirks go in `rust/crates/otterscope/src/ingest` with fixtures, never patched downstream.

## Releases

- Releases are **manual and unsigned**. Tag `vX.Y.Z` on the shipping branch after bumping `[workspace.package] version` in `rust/Cargo.toml` and `CHANGELOG.md`. Then, on each matching host, check out the tag and run `scripts/release.sh --version X.Y.Z`; it builds the frontend and binary, refuses a dirty or version-mismatched tree, and writes `dist/otterscope_X.Y.Z_<artifact>.tar.gz` (`.zip` for Windows). Run it on linux amd64, linux arm64, darwin arm64 and windows amd64; the script derives the target from the host and never assumes a fixed architecture. The four archives, the multi-arch Docker image build/push commands and the Docker-based Linux alternative are in `docs/packaging.md`.
- Build the frontend first so `web/dist` is embedded (the script does this). Linux binaries link glibc, so they are not fully static; see `docs/packaging.md`.
- Pre-1.0: minor = features, patch = fixes. Breaking config/API changes need a migration note in the release notes; a JSON contract change links to `docs/api-rust.md`.
