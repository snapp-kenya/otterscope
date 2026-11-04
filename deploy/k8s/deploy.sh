#!/bin/sh
# Build Otterscope from this working tree, import the image into a local k3d
# cluster, and roll the Deployment to that exact commit. Nothing is pushed off
# this machine.
#
# Usage: deploy/k8s/deploy.sh [--allow-dirty] [--dry-run]
#
# Environment:
#   K3D_CLUSTER      k3d cluster name (default: otterscope)
#   ROLLOUT_TIMEOUT  kubectl rollout timeout (default: 300s)

set -eu

usage() {
    cat <<'EOF'
usage: deploy/k8s/deploy.sh [--allow-dirty] [--dry-run]

Builds the image for HEAD, imports it into a local k3d cluster and applies
deploy/k8s with the image tagged by the short git SHA. Refuses to run when the
working tree is dirty unless --allow-dirty is passed.

  --allow-dirty  deploy an uncommitted tree (the image tag is still HEAD's SHA)
  --dry-run      print the plan without building, importing or applying

Environment:
  K3D_CLUSTER      k3d cluster name (default: otterscope)
  ROLLOUT_TIMEOUT  kubectl rollout timeout (default: 300s)
EOF
}

die() {
    echo "deploy: $*" >&2
    exit 1
}

allow_dirty=0
dry_run=0
for arg in "$@"; do
    case "$arg" in
        --allow-dirty) allow_dirty=1 ;;
        --dry-run) dry_run=1 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown argument: $arg" ;;
    esac
done

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
root=$(CDPATH= cd "$script_dir/../.." && pwd)

for tool in git docker k3d kubectl; do
    command -v "$tool" >/dev/null 2>&1 || die "required command not found: $tool"
done

cluster=${K3D_CLUSTER:-otterscope}
rollout_timeout=${ROLLOUT_TIMEOUT:-300s}
context="k3d-$cluster"

if [ "$allow_dirty" -eq 0 ] && [ -n "$(git -C "$root" status --porcelain)" ]; then
    die "working tree is dirty; commit or stash first (or pass --allow-dirty)"
fi

sha=$(git -C "$root" rev-parse --short HEAD)
[ -n "$sha" ] || die "could not resolve HEAD"
image="otterscope:$sha"

if [ "$dry_run" -eq 1 ]; then
    cat <<EOF
deploy plan (dry run)
  cluster:  $cluster (kubectl context $context)
  commit:   $sha
  image:    $image (local to this machine; never pushed)
  build:    (cd $root && docker build --build-arg version=$sha -t $image .)
  cluster:  k3d cluster create $cluster --servers 1 --agents 0 --wait (if missing)
  import:   k3d image import $image -c $cluster
  render:   kubectl kustomize $root/deploy/k8s
  apply:    replace otterscope:dev with $image, then kubectl apply -f -
  rollout:  kubectl -n otterscope rollout status deployment/otterscope --timeout=$rollout_timeout
  then:     kubectl -n otterscope port-forward svc/otterscope 8080:80
            kubectl -n otterscope exec deploy/otterscope -- otterscope token add admin -admin
            kubectl -n otterscope exec deploy/otterscope -- otterscope token add ui
            kubectl -n otterscope exec deploy/otterscope -- otterscope project add agents
EOF
    exit 0
fi

if k3d cluster list --no-headers 2>/dev/null | awk '{print $1}' | grep -qx "$cluster"; then
    echo "==> using existing k3d cluster $cluster"
else
    echo "==> creating k3d cluster $cluster (1 server, 0 agents)"
    k3d cluster create "$cluster" --servers 1 --agents 0 --wait
fi
kubectl config use-context "$context" >/dev/null

echo "==> building $image from $root"
(
    cd "$root"
    docker build --build-arg "version=$sha" -t "$image" .
)

echo "==> importing $image into k3d cluster $cluster"
k3d image import "$image" -c "$cluster"

echo "==> rendering deploy/k8s and pinning the image tag"
rendered=$(kubectl kustomize "$root/deploy/k8s")
case "$rendered" in
    *"otterscope:dev"*) ;;
    *) die "rendered manifests lack the otterscope:dev placeholder; refusing to apply" ;;
esac
printf '%s\n' "$rendered" | sed "s|otterscope:dev|$image|g" | kubectl apply -f -

echo "==> waiting for rollout of $image"
kubectl -n otterscope rollout status deployment/otterscope --timeout="$rollout_timeout"

cat <<EOF

==> deployed $image

Reach the UI/API with a port-forward (leave it running in another shell):
  kubectl -n otterscope port-forward svc/otterscope 8080:80
  curl -fsS http://127.0.0.1:8080/healthz

Mint the first credentials once; each secret is printed only at creation:
  kubectl -n otterscope exec deploy/otterscope -- otterscope token add admin -admin
  kubectl -n otterscope exec deploy/otterscope -- otterscope token add ui
  kubectl -n otterscope exec deploy/otterscope -- otterscope project add agents

Agents in the cluster send OTLP to:
  OTEL_EXPORTER_OTLP_ENDPOINT=http://otterscope.otterscope.svc.cluster.local
  OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <ingest key>"

The image lives only in this machine's Docker daemon and the k3d cluster.
Upgrade by checking out a new commit and re-running this script.
EOF
