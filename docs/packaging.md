# Packaging

The Rust port ships from `.github/workflows/rust.yml` (CI) and
`.github/workflows/release.yml` (tags). The Go pipeline is retired: the old
`.goreleaser.yaml` is gone, and `.github/workflows/ci.yml` is a manual,
reference-only workflow for the frozen Go tree.

## Toolchain

`rust-toolchain.toml` pins rustc/cargo `1.99.1` with `rustfmt` and `clippy`. CI
and release builds use that file, so local and CI formatting/clippy agree.

## Release targets

`v*` tags build and upload these archives to the GitHub release:

| target | runner | linkage |
| --- | --- | --- |
| linux amd64 | `ubuntu-24.04` | glibc (Ubuntu 24.04 baseline, glibc 2.39) |
| linux arm64 | `ubuntu-24.04-arm` | glibc (Ubuntu 24.04 baseline, glibc 2.39) |
| darwin arm64 | `macos-14` | system libc |
| windows amd64 | `windows-2022` | MSVC, VC++ runtime |

Each archive contains the `otterscope` binary plus `LICENSE` and `README.md`.
The release workflow rewrites the workspace version from the tag before
building, so `otterscope version` reports the released version; a local build
reports the crate version (`0.1.0`). There are no separate checksum files and
no signing.

## Known gaps

- **Linux binaries are not static.** They link the runner's glibc and therefore
  need glibc 2.39 or newer. There is no musl build: a `*-unknown-linux-musl`
  target would need `cross` (or an equivalent sysroot setup), and that is not
  wired up yet. Do not advertise the Linux downloads as static until it is.
- Only the four targets above are built. Darwin amd64, Windows arm64 and
  signing are deferred; Homebrew, deb/rpm and the old `go install` channel
  from ADR-0004 are not shipped (the Go installation path retired with the Go
  module). Homebrew and distro packages can be added to the same matrix later
  if demand justifies the packaging work.
- The arm64 container is compiled under QEMU, so the first multi-arch build is
  slow. It produces the same `linux/amd64,linux/arm64` manifest as the old Go
  pipeline did.

## Container

`Dockerfile` is a three-stage build: Bun builds `web/dist`, Rust compiles
`--release` with the UI embedded by `rust-embed`, and the runtime is
distroless `cc-debian12:nonroot` (digest-pinned; distroless publishes no patch
tags). It runs as uid/gid 65532, keeps `/data` as the only writable path
(`TMPDIR` and `SQLITE_TMPDIR` point there), installs the binary at
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
