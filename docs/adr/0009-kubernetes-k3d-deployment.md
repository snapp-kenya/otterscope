# ADR-0009: Kubernetes via k3d is the only deployment target

Date: 2026-11-04 · Status: accepted · Amends: ADR-0004, ADR-0007

## Context

The operator runs Otterscope on a single machine: an Apple-silicon MacBook Pro
with Docker Desktop, `k3d` and `kubectl`. There is no public deployment, no
hosted instance, no CI service and no container registry the project controls.
The supported runtime is a disposable local Kubernetes cluster with a
`ReadWriteOnce` volume for the one SQLite database.

Earlier documents described a broader surface: a root `docker-compose.yml`, a
`docker run` quick start, pushing an image to a registry, release archives and
marketing-launch drafts under `docs/launch/`. None of that describes how this
project is actually run, and keeping it around invites deployments the operator
does not test.

## Decision

1. **The only supported target is Kubernetes via k3d on the operator's Mac.**
   The manifests are plain YAML plus the kustomize built into `kubectl`
   (`deploy/k8s`); there is no Helm chart, compose file, systemd unit or native
   install path. `deploy/k8s/deploy.sh` is the entrypoint.
2. **Images are local and tagged by the git commit SHA.** `deploy.sh` resolves
   `sha=$(git rev-parse --short HEAD)`, builds
   `otterscope:$sha` with `docker build --build-arg version="$sha"`, imports it
   into the k3d cluster with `k3d image import`, and substitutes the
   `otterscope:dev` placeholder in the rendered manifests. It never pushes an
   image anywhere; there is no registry, no moving tag and no published image.
3. **The SHA is the reported version.** The dockerfile exports
   `OTTERSCOPE_VERSION` from the build arg; `build.rs` compiles it into
   `otterscope::VERSION`. `otterscope version`, `/healthz`'s `version` field and
   the `otterscope_build_info` metric therefore all report the deployed commit.
   A plain `cargo build` falls back to `CARGO_PKG_VERSION`.
4. **A dirty tree is refused by default.** Reproducibility is tied to a commit,
   so `deploy.sh` stops before building unless `--allow-dirty` is passed. That
   flag exists for deliberate local experiments; the image is still tagged with
   `HEAD`'s SHA, so it can be mistaken for a clean build and should not be used
   for a deployment you intend to keep.
5. **TLS, hostnames and exposure belong to the cluster.** The application still
   has one listener on port 80 (ADR-0007); a Service, an optional Ingress or a
   port-forward decides how it is reached. The app terminates no TLS.
6. **Remove the other distribution surface.** `docker-compose.yml` and
   `docs/launch/` are deleted, the readme's quick start is the k3d script, and
   the deploy docs no longer mention a registry, image build-and-push or
   compose. ADR-0004 stays as history with an amendment pointing here.
7. **No release pipeline.** There are no archives, checksums, signatures,
   GitHub Releases, package-manager channels or GHCR image. The local image is
   the artifact, and the deployment is the cluster.

## Consequences

- **No registry, no CI, no publish step.** Deploying requires the working tree
  and a local Docker daemon. The image exists only in Docker's store and in the
  k3d cluster's containerd store; deleting the cluster does not delete the
  Docker image, but nothing is recoverable from a remote.
- **Reproducibility is tied to the working tree.** The tag is only trustworthy
  when the tree was clean. `--allow-dirty` deployments are explicitly not
  reproducible, and `deploy.sh` never claims otherwise.
- **Single writer.** One replica, `Recreate` and a `ReadWriteOnce` PVC remain
  mandatory (ADR-0007 and the deployment manifests). There is no scale-out
  mode, and k3d gives no high availability: the cluster is a development and
  single-operator deployment, not a production control plane.
- **Upgrades and rollbacks are commits.** Upgrade by checking out a newer
  commit and re-running `deploy.sh`; roll back by checking out an earlier one
  and re-running it. The database migration rules still apply: restore the
  pre-upgrade snapshot if the newer binary applied a migration.
- **The cluster is disposable.** `k3d cluster delete` removes the namespace,
  PVC and database. Data leaves the machine only through an explicit backup
  copy.
- The machine's Docker Desktop must be running, and the Mac's architecture is
  the only one built; there is no multi-arch image to publish even if a registry
  were later added.

Reversing this ADR requires a new decision that names the replacement target;
it is not a documentation change.
