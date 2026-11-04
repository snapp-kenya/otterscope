# ADR-0004: Release channels and packaging

Date: 2026-07-14 · Status: amended by ADR-0006 (2026-11-04)

> Amendment note: the body below is the original Go-era decision. What actually ships today — and what is deferred — is in the "Amendment" section at the end of this ADR.

## Context
Distribution is the product's whole thesis (ADR-0001): install must be one command everywhere our users live. The self-hosted community expects an official Docker image and a docker-compose example; developers expect binaries, Homebrew, and `go install`. Because the binary is static and CGO-free, every channel below is buildable from one GitHub Actions release pipeline with no signing infrastructure required at this stage.

## Decision
Tag-triggered (`v*`) GoReleaser pipeline in GitHub Actions publishing, in priority order:

1. **GitHub Releases** — static binaries + checksums for linux/amd64, linux/arm64, darwin/amd64, darwin/arm64, windows/amd64.
2. **Docker image on GHCR** — `ghcr.io/otterscope/otterscope`, multi-arch (amd64/arm64), `FROM scratch` + binary + CA certs; tags `latest`, `X.Y.Z`, `X.Y`. A `docker-compose.yml` example lives in the repo root.
3. **Homebrew tap** — `otterscope/homebrew-tap`, formula auto-bumped by GoReleaser.
4. **deb/rpm via nfpm** (inside GoReleaser) attached to GitHub Releases; a proper apt repo only if demand appears.
5. **`go install github.com/otterscope/otterscope/cmd/otterscope@latest`** — works implicitly; keep the module path stable.

Deferred until users ask: Windows signing/Scoop/winget, apt/yum hosted repos, Nix, Helm chart (a plain K8s manifest example ships in docs instead).

## Consequences
- Release cost per version is one git tag; everything else is CI.
- The Docker image must stay `FROM scratch`-compatible: no runtime file dependencies beyond the binary and the SQLite data dir — this constrains features the same way ADR-0001 does.
- Version is injected via `-ldflags -X main.version=`; `otterscope version` must always report accurately.

## Amendment (2026-11-04): what actually ships

The body above is the original Go-era decision and is kept as history. After the Rust port (ADR-0006), releases are **built and published manually; there is no CI**. The GitHub Actions workflows were deleted by operator decision, and `scripts/release.sh` plus `docs/packaging.md` are the source of truth for this amendment.

**Shipping today:**

1. **GitHub Releases** — four native archives, built on matching hosts and uploaded by hand, not five platforms and not static binaries: linux amd64, linux arm64, darwin arm64 and windows amd64. Each archive contains the `otterscope` binary, `LICENSE` and `README.md`. There are no separate checksum files and no signing.
2. **Docker image on GHCR** — one multi-arch `ghcr.io/otterscope/otterscope` image for linux/amd64 and linux/arm64, tagged `latest` and `X.Y.Z`, built and pushed manually with `docker buildx`. The runtime is distroless `cc-debian12:nonroot` (uid 65532, `/data` as the only writable path), not `FROM scratch`; the body's `X.Y` tag is not published.
3. **Versioning** — the release commit sets `[workspace.package] version` in `rust/Cargo.toml` (and `Cargo.lock`), and the release script refuses to build when it differs from the tag; the Docker build receives the same value as `--build-arg version`. `otterscope version` reports the released version. There is no `-ldflags` injection.

**Deferred or dropped:**

- **Linux linkage:** the Linux artifacts link glibc 2.39+ when built on the Ubuntu 24.04 baseline (2.36 via the Debian bookworm Docker path). There is no musl/cross build yet, so do not describe them as static.
- **Homebrew tap:** deferred. It needs a separate tap repo and formula maintenance; there is no tap today.
- **deb/rpm packages:** deferred. They need nfpm or equivalent packaging plus distro testing; no distro packages are attached to releases.
- **darwin amd64 and windows arm64:** deferred. The manual release procedure builds only the four targets above; each extra target needs its own host and testing.
- **Signing (Windows/macOS notarization), Scoop, winget, Nix and Helm:** deferred for the same reasons — certificates, extra packaging and maintenance without current demand.
- **`go install`:** retired with the Go module; the Rust workspace is not installable through the Go toolchain.

Why: the four native targets and the Docker image cover Linux, macOS and Windows without requiring signing infrastructure or per-distro packaging, which keeps a release to one tag plus a short manual checklist. Homebrew, distro packages and the extra targets are real distribution work, not a line in a YAML file; they can be added to the same manual release procedure when demand justifies the maintenance.

Reversing any of this requires a new ADR or an amendment here; the historical body is not a promise that the deferred channels exist.
