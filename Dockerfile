# Built by GoReleaser: the context contains the prebuilt static binary.
# FROM scratch is a product constraint (ADR-0004): no runtime dependencies
# beyond the binary, CA certs (for LLM-judge evals), and the data volume.
FROM alpine:3.22 AS certs
RUN apk add --no-cache ca-certificates \
 && mkdir -p /data \
 && chown 65532:65532 /data

FROM scratch
COPY --from=certs /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --chmod=0555 otterscope /otterscope

# /data is the only writable path: the SQLite database, its WAL/journal
# siblings, and any temp files SQLite needs all land here. It ships in the
# image owned by the runtime uid so the server can create its own database
# without a root pre-step; a fresh named volume inherits that ownership, while
# a host bind mount must be chowned to 65532 on the host.
COPY --from=certs --chown=65532:65532 /data /data
ENV TMPDIR=/data \
    SQLITE_TMPDIR=/data
WORKDIR /data
VOLUME ["/data"]

# Non-root by default: uid/gid 65532 is the "nonroot" convention (distroless).
# Combined with scratch's lack of a shell and the single writable mount, this
# lets the container run with --read-only and no added capabilities.
USER 65532:65532

EXPOSE 8317 4318

# Deliberately no -listen/-otlp: the binary's own defaults are loopback
# (127.0.0.1:8317 and 127.0.0.1:4318), which is the right posture for an image
# meant to be used locally. A deployment that must accept connections through
# the container's network namespace (docker compose, Kubernetes) passes
# -listen/-otlp explicitly and owns the exposure decision itself.
ENTRYPOINT ["/otterscope"]
CMD ["serve", "-db", "/data/otterscope.db"]
