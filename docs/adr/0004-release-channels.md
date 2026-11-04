# ADR-0004: Release channels and packaging

Date: 2026-07-14 · Status: amended by ADR-0006 (2026-11-04)

> Amendment note: the body below is the original Go-era decision and is history, not a description of what this repository does. Release channels are withdrawn; see the "Amendment" section at the end, which is the only part that describes today.

## Context
Distribution is the product's whole thesis (ADR-0001): install must be one command everywhere our users live. The self-hosted community expects an official Docker image and a docker-compose example; developers expect binaries, Homebrew, and `go install`. Because the binary is static and CGO-free, every channel below is buildable from one GitHub Actions release pipeline with no signing infrastructure required at this stage.

## Decision
Tag-triggered (`v*`) GoReleaser pipeline in GitHub Actions publishing, in priority order:

1. **GitHub Releases** — static binaries + checksums for linux/amd64, linux/arm64, darwin/amd64, darwin/arm64, windows/amd64.
2. **Docker image on GHCR** — a multi-arch (amd64/arm64) GitHub Container Registry image, `FROM scratch` + binary + CA certs; tags `latest`, `X.Y.Z`, `X.Y`. A `docker-compose.yml` example lives in the repo root.
3. **Homebrew tap** — `otterscope/homebrew-tap`, formula auto-bumped by GoReleaser.
4. **deb/rpm via nfpm** (inside GoReleaser) attached to GitHub Releases; a proper apt repo only if demand appears.
5. **`go install github.com/otterscope/otterscope/cmd/otterscope@latest`** — works implicitly; keep the module path stable.

Deferred until users ask: Windows signing/Scoop/winget, apt/yum hosted repos, Nix, Helm chart (a plain K8s manifest example ships in docs instead).

## Consequences
- Release cost per version is one git tag; everything else is CI.
- The Docker image must stay `FROM scratch`-compatible: no runtime file dependencies beyond the binary and the SQLite data dir — this constrains features the same way ADR-0001 does.
- Version is injected via `-ldflags -X main.version=`; `otterscope version` must always report accurately.

## Amendment (2026-11-04): every channel is withdrawn

The body above is the original Go-era decision and is kept as history, not as a
promise: it does not describe what this repository does today.

**Operator decision (2026-11-04): all release channels are withdrawn.** The
channels described above — download archives, Homebrew, deb/rpm, signing and
the GHCR image — are withdrawn. There is nothing to download: no archives, no
checksums, no signatures, no GitHub Releases, no GHCR image and no
package-manager distribution. Homebrew, deb/rpm, `go install`, Scoop, winget,
Nix and a Helm chart are not deferred work that may return — they are dropped.
The manual archive script and packaging document that briefly existed after the
Rust port (ADR-0006) have been deleted.

**The only artifact is the container image.** It is built from the repository's
`dockerfile` and pushed by the deploying team to **their own** registry; this
repository publishes nothing. Build it with `docker build -t otterscope .` and
deploy it as [`deploy/readme.md`](../../deploy/readme.md) describes. There is no
substitute release process, no tag-and-upload checklist and no channel to
restore.

**What still holds:**

- **Versioning** — `[workspace.package] version` in `rust/Cargo.toml` (and
  `Cargo.lock`) is the single version; `otterscope version` reports it, and the
  Docker build can receive the same value as `--build-arg version`. There is no
  `-ldflags` injection.
- **Linux linkage** — a Linux build links glibc 2.39+ on the Ubuntu 24.04
  baseline (2.36 via the Debian bookworm image build). There is no musl build,
  so do not describe it as static; the container image is the portable option.
- The original body's `FROM scratch`, static-binary, multi-platform and
  CI-pipeline description is Go-era history.

Reversing any of this requires a new ADR; the historical body is not a promise
that the withdrawn channels exist.
