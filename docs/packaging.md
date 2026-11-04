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

`docker-compose.yml` keeps the deployment posture of the Go example: loopback
publishing, read-only root filesystem, all capabilities dropped, named data
volume, and read auth on. Mint the first read or admin token and a project
ingest key with the `docker compose run` commands documented in that file.
