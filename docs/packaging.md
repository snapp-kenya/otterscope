# Packaging

Otterscope has **no CI**. Releases are built and published manually from a
clean checkout of the release tag, on a host that matches each target, or — for
the two Linux targets — in Docker. `scripts/release.sh` automates one native
archive per invocation on the host it runs on; the multi-arch container image is
a separate manual `docker buildx` push. Nothing is signed: no checksums, no
Windows code signing, no macOS notarization. The local quality gate in
[`development.md`](development.md) must pass before the tag is cut.

The old Go pipeline and its manual CI workflow were deleted with the Go tree;
the Rust workspace and `scripts/release.sh` are the only build system.

## Toolchain

`rust-toolchain.toml` pins rustc/cargo `1.99.1` with `rustfmt` and `clippy`.
Releases are built with Bun 1.4.2; `web/bun.lock` pins the dependencies but not
the Bun runtime, so check `bun --version` on a new release host. Local builds
use the same files, so there is no CI/local drift to reconcile — but there is
also no CI to notice when a host differs, which is why the gate below is run
manually before tagging:

```sh
cd rust && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
cd web && bun run lint && bun test && bun run build
```

## Versioning

The version lives in `rust/Cargo.toml` under `[workspace.package] version`.
`CARGO_PKG_VERSION` (exposed as `otterscope::VERSION` and printed by
`otterscope version`) bakes it into the binary at compile time; there is no
`-ldflags` injection.

A release commit bumps that one field (plus `rust/Cargo.lock`, which records
the workspace package version) and `changelog.md`, then tags it `vX.Y.Z`. The
release script refuses to build unless `[workspace.package] version` equals the
`--version` passed to it, so a tag can never ship a binary that reports a
different version. The Docker image gets the same value as
`--build-arg version=...`, which rewrites the workspace version inside the image
build; archives and image therefore agree.

## Release procedure

### 1. Cut the release commit and tag

```sh
VERSION=0.2.0
# edit rust/Cargo.toml: [workspace.package] version = "0.2.0"
# edit changelog.md: move "Unreleased" entries under the new version
cd rust && cargo check && cd ..          # refreshes Cargo.lock's version
git add rust/Cargo.toml rust/Cargo.lock changelog.md
git commit -m "Release v$VERSION"
git tag -a "v$VERSION" -m "v$VERSION"
git push origin main --follow-tags
```

`git status` must be clean afterwards: the release script refuses a dirty tree,
because a local edit or leftover artifact means the archive would not be built
from the tag.

### 2. Build one native archive per target

Check out the tag on each host, then run the script. It derives the Rust target
triple from `rustc -vV` for the current host, so the same command works on every
architecture; it never assumes amd64. `--target` overrides the triple, and
`--artifact` overrides the archive suffix if a new target is added.

| archive | host | command |
| --- | --- | --- |
| `linux-amd64` | x86_64 Linux | `scripts/release.sh --version 0.2.0` |
| `linux-arm64` | arm64 Linux | `scripts/release.sh --version 0.2.0` |
| `darwin-arm64` | Apple silicon macOS | `scripts/release.sh --version 0.2.0` |
| `windows-amd64` | Windows (Git Bash, `zip` on `PATH`) | `scripts/release.sh --version 0.2.0` |

Common flags:

```text
--version X.Y.Z     release version, without the leading v (required)
--target TRIPLE     cargo target triple (default: this host's triple)
--artifact NAME     archive suffix (default: derived from the target)
--dist DIR          output directory (default: dist)
--dry-run           check the tree/version and print the plan, build nothing
-h, --help          usage
```

The script builds `web/dist` (`bun install --frozen-lockfile && bun run build`),
then `cargo build --release --locked` for the target, stages the binary with
`license` and `readme.md`, and writes
`dist/otterscope_<version>_<artifact>.tar.gz` (`.zip` on Windows). At the end it
prints the GitHub release and GHCR commands; it never pushes anything itself.

On Windows without Git Bash, the equivalent PowerShell steps are:

```powershell
$Version = "0.2.0"
cd web; bun install --frozen-lockfile; bun run build
cd ..\rust; cargo build --release --locked --target x86_64-pc-windows-msvc
cd ..
$Name = "otterscope_${Version}_windows-amd64"
New-Item -ItemType Directory -Force "dist/$Name" | Out-Null
Copy-Item "rust\target\x86_64-pc-windows-msvc\release\otterscope.exe" "dist/$Name/"
Copy-Item license, readme.md "dist/$Name/"
Compress-Archive -Path "dist/$Name" -DestinationPath "dist/$Name.zip"
```

### 3. Publish the archives

Upload the four archives to the GitHub release for the tag. The release is
unsigned and has no checksum files:

```sh
gh release create "v$VERSION" dist/* --title "v$VERSION" --generate-notes
# if the release already exists:
gh release upload "v$VERSION" dist/* --clobber
```

Pass `--prerelease` for a pre-release tag (`vX.Y.Z-rc.N`). Build the other three
archives before publishing, or upload them as they arrive; a missing host means
a missing archive, not a failed check.

### 4. Build and push the container image

One multi-arch image for linux/amd64 and linux/arm64, tagged `latest` and
`X.Y.Z` (no `X.Y` tag). This requires `docker buildx` and, for the arm64 half,
QEMU/binfmt on an amd64 host:

```sh
echo "$GHCR_TOKEN" | docker login ghcr.io -u <github-user> --password-stdin
docker buildx create --name otterscope --use   # once per machine
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  --build-arg version="$VERSION" \
  -t "ghcr.io/otterscope/otterscope:$VERSION" \
  -t ghcr.io/otterscope/otterscope:latest \
  --push .
```

The arm64 half is compiled under QEMU, so the first build is slow; later builds
reuse the layer cache. The `dockerfile` is a three-stage build (Bun → Rust →
distroless runtime); see [Container](#container) for the runtime posture.

### 5. Verify the release

```sh
./otterscope version                     # reports 0.2.0 for a downloaded archive
docker run --rm ghcr.io/otterscope/otterscope:"$VERSION" version
```

## What goes in each archive

Each archive contains a single top-level directory,
`otterscope_<version>_<artifact>/`, with:

- the `otterscope` binary (or `otterscope.exe` on Windows),
- `license`,
- `readme.md`.

There are no separate checksum files and no signatures. Archives are `tar.gz`
on Linux and macOS, `zip` on Windows.

## Release targets and linkage

| target | build host | linkage |
| --- | --- | --- |
| linux amd64 | x86_64 Linux (Ubuntu 24.04 baseline) | glibc, not static |
| linux arm64 | arm64 Linux (Ubuntu 24.04 baseline) | glibc, not static |
| darwin arm64 | Apple silicon macOS | system libc |
| windows amd64 | Windows with MSVC | MSVC, VC++ runtime |

Linux binaries link the build host's glibc: Ubuntu 24.04 gives glibc 2.39, the
Debian bookworm Docker path gives 2.36. None of them are static, and the
minimum glibc is therefore a property of the host that built the archive. Do not
advertise the Linux downloads as static.

## Building a Linux archive in Docker (no matching host)

The `dockerfile` already builds the frontend and the binary for the requested
platform, with the version injected. Build for one platform, extract the binary
from a container, and package it on the host. This works for either Linux
architecture; on an amd64 host the arm64 build runs under QEMU.

```sh
VERSION=0.2.0
ARCH=amd64           # or arm64
docker buildx build --platform "linux/$ARCH" \
  --build-arg version="$VERSION" \
  -t "otterscope-archive:$ARCH" --load .
cid=$(docker create --platform "linux/$ARCH" "otterscope-archive:$ARCH")
name="otterscope_${VERSION}_linux-${ARCH}"
mkdir -p "dist/$name"
docker cp "$cid:/usr/local/bin/otterscope" "dist/$name/otterscope"
docker rm "$cid"
cp license readme.md "dist/$name/"
tar -czf "dist/${name}.tar.gz" -C dist "$name"
rm -rf "dist/$name"
```

For emulated arm64, install binfmt once with
`docker run --privileged --rm tonistiigi/binfmt --install arm64`. The resulting
binary is the one the container runs (distroless-compatible, glibc 2.36), so it
is a valid archive but not byte-identical to a host build. Docker cannot produce
the macOS or Windows archives: those need their native toolchains and SDKs, so
they must be built on matching hosts.

## Known gaps

- **Linux binaries are not static.** They link the build host's glibc (2.39 on
  Ubuntu 24.04, 2.36 via the bookworm Docker image) and there is no musl/cross
  build. A `*-unknown-linux-musl` target would need `cross` (or an equivalent
  sysroot setup) and is not wired up.
- Only the four targets above are built. Darwin amd64, Windows arm64 and
  signing are deferred; Homebrew and deb/rpm are not shipped. The Go-era
  installation channel from ADR-0004 retired with the Go module and is not
  coming back. These can be added to the same manual procedure later if demand
  justifies the packaging work.
- **The process is manual.** There is no tag-triggered automation: forgetting a
  host, a version bump, or the GHCR push produces no artifact and no alarm. The
  script catches dirty trees and version mismatches; everything else is on the
  release checklist above.
- The arm64 container is compiled under QEMU, so the first multi-arch build is
  slow.

## Container

`dockerfile` is a three-stage build: Bun builds `web/dist`, Rust compiles
`--release` with the UI embedded by `rust-embed`, and the runtime is distroless
`cc-debian12:nonroot` (digest-pinned; distroless publishes no patch tags). It
runs as uid/gid 65532, keeps `/data` as the only writable path (`TMPDIR` and
`SQLITE_TMPDIR` point there), installs the binary at
`/usr/local/bin/otterscope`, and defaults to `serve -db /data/otterscope.db`.
`docker run <image> version` still works because the entrypoint is the binary
and `serve` is the default command.

The image exposes exactly one port, **80**: the server's single listener for
the UI, JSON API, `/mcp`, share links and OTLP ingest. The image stays non-root
and never switches to root. Under Docker, uid 65532 can bind 80 without
`CAP_NET_BIND_SERVICE` because the runtime sets
`net.ipv4.ip_unprivileged_port_start=0`; Kubernetes/containerd keeps the default
boundary at 1024, so those deployments need the capability (the manifests add
it). Native runs need root or the capability. The `docker run` example below
still drops all capabilities and adds only `NET_BIND_SERVICE`, which is
harmless under Docker and keeps the command portable; choose the host-side
mapping yourself (for example `-p 8080:80`; publishing is a platform decision,
the process always listens on 80 inside the container):

```sh
docker run --read-only --cap-drop ALL --cap-add NET_BIND_SERVICE \
  -p 127.0.0.1:8080:80 -v otterscope:/data \
  ghcr.io/otterscope/otterscope serve -db /data/otterscope.db
```

`docker-compose.yml` uses the same posture: loopback publishing
(`127.0.0.1:8080:80`), read-only root filesystem, all capabilities dropped
except `NET_BIND_SERVICE`, named data volume, and read auth on. Mint the first
read or admin token and a project ingest key with the `docker compose run`
commands documented in that file.

### Base path

The stock image is built for the root. A deployment under a path prefix — for
example `serve -base-path /otterscope` behind a path-based Ingress — **requires
an image built with the matching build argument**, because Vite bakes the UI's
router and API base into the JavaScript:

```sh
docker build --build-arg OTTERSCOPE_BASE_PATH=/otterscope -t <img> .
```

The server still starts when `-base-path` and the baked base disagree; the API,
OTLP receiver and MCP remain valid. It logs a prominent `WARN` naming the build
arg to use, and the UI will not work because its router and API calls target
the other base. `deploy/k8s/ingress.yaml` documents the rest of the prefixed
setup (configuration, probe paths and the Ingress rule).
