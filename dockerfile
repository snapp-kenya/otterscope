from --platform=$BUILDPLATFORM docker.io/oven/bun:1.4.2-alpine as web

# The UI bakes its API/router base path into the JavaScript bundle, so a
# path-based Ingress needs an image built with the same prefix the server is
# configured with at runtime. The default keeps the root-mounted image.
arg OTTERSCOPE_BASE_PATH=""

workdir /src/web

copy web/package.json web/bun.lock ./
run bun install --frozen-lockfile

copy web ./
run OTTERSCOPE_BASE_PATH="${OTTERSCOPE_BASE_PATH}" bun run build

from docker.io/library/rust:1.99.1-bookworm as rust

workdir /src/rust

copy rust/Cargo.toml rust/Cargo.lock ./
copy rust/crates/otterscope/Cargo.toml crates/otterscope/Cargo.toml
copy rust/crates/otterscope/build.rs crates/otterscope/build.rs

run mkdir -p crates/otterscope/src \
 && printf 'fn main() {}' > crates/otterscope/src/main.rs \
 && printf '' > crates/otterscope/src/lib.rs \
 && cargo build --release

copy rust/crates/otterscope ./crates/otterscope
copy --from=web /src/web/dist /src/web/dist

arg version
run if [ -n "$version" ]; then sed -i -E "s/^version = \"[^\"]+\"/version = \"$version\"/" Cargo.toml; fi \
 && touch crates/otterscope/src/main.rs crates/otterscope/src/lib.rs \
 && cargo build --release \
 && mkdir -p /data \
 && chown 65532:65532 /data

from gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f

copy --from=rust /src/rust/target/release/otterscope /usr/local/bin/otterscope
copy --from=rust --chown=65532:65532 /data /data

env TMPDIR=/data \
    SQLITE_TMPDIR=/data
workdir /data
volume ["/data"]
user 65532:65532
# The server always listens on port 80. Docker lets uid 65532 bind it without
# extra capabilities (it sets net.ipv4.ip_unprivileged_port_start=0);
# Kubernetes/containerd keeps 1024 privileged and needs NET_BIND_SERVICE:
#   docker run --cap-drop ALL --cap-add NET_BIND_SERVICE -p 8080:80 ...
expose 80
entrypoint ["/usr/local/bin/otterscope"]
cmd ["serve", "-db", "/data/otterscope.db"]
