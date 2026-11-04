# Deploying Otterscope on Kubernetes

Plain YAML plus `kustomize` (the one built into `kubectl`). There is no Helm
chart, no chart dependency, and no templating step. `kubectl apply -k
deploy/k8s` is the whole install.

> **One writer. One replica. No horizontal scaling.**
>
> Otterscope stores everything in one embedded SQLite database. SQLite has a
> single writer, and the store holds a single connection to it, so exactly one
> Otterscope process may own a database at a time. The manifests therefore use
> `replicas: 1`, `strategy: Recreate`, and a `ReadWriteOnce` PVC. Do **not**
> raise `replicas`, add a HorizontalPodAutoscaler, switch to `RollingUpdate`,
> or mount one PVC into two Otterscope pods. `ReadWriteOnce` is also a data
> safety requirement: SQLite locking is not reliable on NFS/RWX storage, even
> if every pod is read-only. If one process cannot carry the ingest load, run
> independent instances, each with its own database and PVC, and split agents
> between them; there is no shared-database scale-out mode.

The whole application listens on **port 80 only**: UI, JSON API, `/mcp`,
`/healthz`, public share links and OTLP/HTTP ingest (`/v1/traces`) share that
one port. TLS and external hostnames are the job of the ingress controller or
Istio, not of this Deployment. [ADR-0007](../docs/adr/0007-one-port-80.md)
records that decision.

## What the kustomization creates

| File | Resource |
| --- | --- |
| `namespace.yaml` | namespace `otterscope`, Pod Security Standard `restricted` |
| `serviceaccount.yaml` | `otterscope` ServiceAccount, no API token automount |
| `pvc.yaml` | 10Gi `ReadWriteOnce` PVC `otterscope-data` |
| `configmap.yaml` | `OTTERSCOPE_*` runtime configuration |
| `deployment.yaml` | one replica, port 80, probes, resource limits, hardened pod |
| `service.yaml` | ClusterIP Service on port 80 |
| `cronjob.yaml` | daily `otterscope backup` snapshot |
| `ingress.yaml` | **optional, disabled by default** path-based example |
| `secret.example.yaml` | **optional, not applied** judge key example |

There is no separate OTLP Service or port. Agents point at the same hostname
and port as the UI; the path (`/v1/traces`) selects ingest.

## Prerequisites

- Kubernetes 1.27 or newer (the CronJob uses the stable `timeZone` field).
- A cluster with a default `ReadWriteOnce` StorageClass. If the default class
  is not durable or does not support `fsGroup`, set `storageClassName` in
  `pvc.yaml` explicitly. The pod runs as uid/gid 65532 and relies on
  `fsGroup: 65532` (or a pre-provisioned group-writable volume) to create the
  database and WAL files.
- `kubectl` (its embedded kustomize is enough) and the image
  `ghcr.io/otterscope/otterscope:latest`. Pin a release tag or digest in
  `deployment.yaml` and `cronjob.yaml` for production; both must run the same
  version because the backup command opens the database and applies/migrates
  the schema the same way `serve` does.
- No Helm. Do not add one.

## Install

```sh
# From the repository root. A client-side dry run needs no namespace yet:
kubectl apply --dry-run=client -k deploy/k8s

kubectl apply -k deploy/k8s
kubectl -n otterscope rollout status deployment/otterscope --timeout=5m
kubectl -n otterscope get deploy,pod,svc,pvc
```

For a server-side admission check, create the namespace first (the
kustomization includes it, so the dry-run cannot create it implicitly):
`kubectl create namespace otterscope` then
`kubectl apply --dry-run=server -k deploy/k8s`.

Reach the UI/API locally with a port-forward (the Service port is 80; the
local port can be anything unprivileged):

```sh
kubectl -n otterscope port-forward svc/otterscope 8080:80
# in another shell:
curl -fsS http://127.0.0.1:8080/healthz
# {"status":"ok","version":"..."}
```

`/healthz` is public even with read auth on. It is also what the startup,
readiness and liveness probes use.

## First token and first ingest key

`OTTERSCOPE_READ_AUTH=true` is the default in `configmap.yaml` and is the
correct posture outside loopback. The UI and `/api` need a Bearer token; OTLP
ingest needs a project ingest key. Both are minted by the CLI against the same
SQLite database.

Run the CLI **inside the running server pod** with `kubectl exec`. That is
deliberate: it is the same process namespace, the same node, and the same
database connection file as the writer, so it does not create a second pod
competing for the `ReadWriteOnce` volume. The `OTTERSCOPE_DB` value comes from
the ConfigMap, so `-db` is not needed.

```sh
# Admin token: mint/revoke tokens, assertions, saved views, alerts, evals.
kubectl -n otterscope exec deploy/otterscope -- \
  otterscope token add admin -admin

# Read token: the UI/dashboard query surface.
kubectl -n otterscope exec deploy/otterscope -- \
  otterscope token add ui

# Project + ingest key for agents.
kubectl -n otterscope exec deploy/otterscope -- \
  otterscope project add agents
```

Each command prints the secret **once**; `otterscope token list` and `project
list` never print secrets again. Store them in your secret manager. To capture
them in a script:

```sh
ADMIN_TOKEN="$(kubectl -n otterscope exec deploy/otterscope -- \
  otterscope token add admin -admin | awk '/^token: /{print $2}')"
UI_TOKEN="$(kubectl -n otterscope exec deploy/otterscope -- \
  otterscope token add ui | awk '/^token: /{print $2}')"
INGEST_KEY="$(kubectl -n otterscope exec deploy/otterscope -- \
  otterscope project add agents | awk '/^ingest key: /{print $3}')"
```

`token add` is idempotent only in the sense that it mints another token each
time; it does not reuse a name. Run it once per intended credential and keep
the output. Avoid `set -x` around these commands.

There is intentionally no bootstrap Job in the kustomization. A Job that mints
a token on every `kubectl apply` would create a new credential per sync, and a
Job pod would be a second pod on the single-writer volume. If you need this in
GitOps, make it a one-shot Job that you add, run, and delete explicitly, with
the token written to a Secret.

The optional judge key is a normal Secret, not an Otterscope-managed token:

```sh
cp deploy/k8s/secret.example.yaml /tmp/judge-secret.yaml
$EDITOR /tmp/judge-secret.yaml          # replace the placeholder
kubectl -n otterscope apply -f /tmp/judge-secret.yaml
kubectl -n otterscope rollout restart deployment/otterscope
```

The Deployment references the key with `optional: true`, so the server starts
without the Secret and the judge feature stays unconfigured.

## Agents: the OTLP endpoint

In-cluster, agents use the Service and port 80 (the port is implied):

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT=http://otterscope.otterscope.svc.cluster.local
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${INGEST_KEY}"
```

The exporter appends `/v1/traces` itself, so the request lands on
`http://otterscope.otterscope.svc.cluster.local/v1/traces`. For an agent
running on your laptop, port-forward and use the local port:

```sh
kubectl -n otterscope port-forward svc/otterscope 8080:80
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:8080
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${INGEST_KEY}"
```

If the instance is mounted under a base path (see below), append that prefix to
the endpoint; OTLP is then at `/otterscope/v1/traces`. One endpoint serves the
UI and ingest, so there is no separate ingest URL to publish.

With `read-auth` off, keyless ingest falls into the `default` project. Do not
turn read auth off on a cluster that is reachable by anything you do not
control: it also opens the whole read/write API.

## Auth, origins and TLS

- Keep `OTTERSCOPE_READ_AUTH: "true"`. The token scopes are read and admin;
  mutating routes need admin.
- If the UI is reached through a hostname (Ingress, Istio, load balancer), add
  the external origin to `OTTERSCOPE_ALLOWED_ORIGINS` in `configmap.yaml`,
  e.g. `https://otterscope.example.com`, then restart the Deployment. Without
  it the browser request is refused with `403 {"error":"host not allowed"}` or
  `{"error":"origin not allowed"}`. Loopback and IP literals are accepted
  without an entry.
- TLS is terminated outside this Deployment. `ingress.yaml` has no certificate
  of its own; point `tls.secretName` at cert-manager or your controller, or put
  an Istio gateway in front. The Service is ClusterIP and deliberately not
  exposed with `NodePort` or `LoadBalancer`.

## Optional path-based Ingress (disabled by default)

`ingress.yaml` is not in `kustomization.yaml`. It demonstrates serving the
whole app under `/otterscope` with one path rule and no rewrite annotation:
the app strips the prefix itself. Enabling it needs all of these, because a
base path is a build-time + runtime setting, not just an Ingress rewrite:

1. **Build the image with the same prefix.** The React bundle bakes its API
   and router base into the JavaScript at build time:

   ```sh
   docker build --build-arg OTTERSCOPE_BASE_PATH=/otterscope \
     -t registry.example/otterscope:basepath .
   ```

   A root-built release image under this Ingress serves the HTML but its JS
   calls `/api`, which the Ingress does not route. The server rewrites asset
   URLs in `index.html` at request time, but not the JS bundle.
2. Set `OTTERSCOPE_BASE_PATH: "/otterscope"` and
   `OTTERSCOPE_ALLOWED_ORIGINS: "https://otterscope.example.com"` in
   `configmap.yaml`.
3. Change all three probe paths in `deployment.yaml` from `/healthz` to
   `/otterscope/healthz`.
4. Uncomment `ingress.yaml` in `kustomization.yaml`, or
   `kubectl apply -f deploy/k8s/ingress.yaml` after editing the host.

Then the UI/API is at `https://otterscope.example.com/otterscope/` and the
OTLP endpoint is `https://otterscope.example.com/otterscope` (the exporter
appends `/v1/traces`). External agents also need to send the project ingest
key. A path rewrite to `/` is wrong for this setup: the app's emitted URLs
already carry the prefix.

## Backups

`cronjob.yaml` runs `otterscope backup` daily at 03:17 UTC. It writes a
consistent snapshot to the PVC:

```text
/data/backups/otterscope-<pod-name>.db
```

The pod name is unique per attempt, so a retried Job does not collide with an
existing file. The backup subcommand creates the `backups` directory itself;
the image is distroless and has no shell or `mkdir`. The Job runs on the same
node as the server (`podAffinity`) because the PVC is `ReadWriteOnce`; if the
server pod is absent, the backup waits rather than failing on a multi-attach.

Run one now and inspect it:

```sh
kubectl -n otterscope create job --from=cronjob/otterscope-backup \
  otterscope-backup-manual
kubectl -n otterscope wait --for=condition=Complete \
  job/otterscope-backup-manual --timeout=10m
kubectl -n otterscope logs job/otterscope-backup-manual
# backed up /data/otterscope.db to /data/backups/otterscope-....db
```

The image has no `ls`, so list the directory from a temporary pod that mounts
the same PVC on the writer's node (the pod affinity keeps the `ReadWriteOnce`
volume attachable):

```sh
kubectl -n otterscope run pvc-inspect --rm -it --restart=Never \
  --image=busybox:1.37 \
  --overrides='{"spec":{"automountServiceAccountToken":false,"affinity":{"podAffinity":{"requiredDuringSchedulingIgnoredDuringExecution":[{"labelSelector":{"matchLabels":{"app.kubernetes.io/name":"otterscope","app.kubernetes.io/component":"server"}},"topologyKey":"kubernetes.io/hostname"}]}},"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"fsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"pvc-inspect","image":"busybox:1.37","command":["ls","-ln","/data/backups"],"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"volumeMounts":[{"name":"data","mountPath":"/data"}]}],"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"otterscope-data"}}]}}'
```

Two operational facts the manifest cannot hide:

- **Snapshots accumulate on the same volume.** The CronJob does not prune
  files, and the distroless image has no `rm`. Copy snapshots to object
  storage or a backup PVC on your own schedule, and monitor free space.
  `OTTERSCOPE_RETENTION` bounds the database's own growth, not the backup
  directory's.
- **The backup process opens the database.** It must use the same image
  version as the server; a newer backup image can apply a pending migration
  that the running server does not understand. `kustomize edit set image`
  updates both the Deployment and the CronJob because they share the image
  name.

### Pre-upgrade snapshot and restore

The server already snapshots an existing database next to itself before
applying pending migrations (`/data/otterscope.db.pre-migrate-<ns>.bak`, only
the newest is kept). Take an explicit snapshot with a name you can find:

```sh
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
kubectl -n otterscope exec deploy/otterscope -- \
  otterscope backup -o "/data/backups/pre-upgrade-${STAMP}.db"
```

A database migrated by a newer version cannot be opened by an older one, so a
rollback is restore-then-rollback, never just an older image on the migrated
file. To restore: scale the Deployment to 0, replace `/data/otterscope.db` with
the chosen snapshot from a temporary pod (and remove the stale
`otterscope.db-wal`/`otterscope.db-shm`), then scale back to 1 and confirm
`/healthz` and a known run. Do this with the writer stopped; never swap the
file under a running server.

## Upgrades

`strategy: Recreate` means a short outage on every rollout: the old pod is
stopped before the new one starts, so there is never a second writer on the
volume.

```sh
# 1. Snapshot first (see above).
# 2. Change the image field in deployment.yaml and cronjob.yaml. The standalone
#    kustomize CLI can do both with:
#      kustomize edit set image \
#        ghcr.io/otterscope/otterscope=ghcr.io/otterscope/otterscope:vX.Y.Z
# 3. Apply and watch.
kubectl apply -k deploy/k8s
kubectl -n otterscope rollout status deployment/otterscope --timeout=5m
kubectl -n otterscope get pods -w
```

Readiness gates on `/healthz`; the startup probe allows up to 300 seconds for a
large database to migrate. If migration fails, the pod crash-loops and the
pre-migration snapshot is the recovery point. Keep the previous image tag and
snapshot until the new version is confirmed.

## Security posture

The manifests are compatible with the namespace's `restricted` Pod Security
Standard:

- `runAsNonRoot: true`, `runAsUser`/`runAsGroup`/`fsGroup: 65532`;
- `readOnlyRootFilesystem: true`, with `/data` (the PVC) as the only writable
  path; `TMPDIR` and `SQLITE_TMPDIR` already point at `/data`;
- `allowPrivilegeEscalation: false`, capabilities `drop: [ALL]`;
- `NET_BIND_SERVICE` is the single capability added, only for the server
  container, because uid 65532 binds port 80. The backup container does not
  bind and does not get it. If your cluster rejects that capability, the
  alternative is the safe sysctl `net.ipv4.ip_unprivileged_port_start: "0"` in
  the pod `securityContext` where the kubelet allows it; do not solve it by
  running as root;
- `seccompProfile: RuntimeDefault` at pod level;
- `automountServiceAccountToken: false` on the ServiceAccount and every pod;
- no RBAC objects; the workload never calls the Kubernetes API.

No NetworkPolicy is included: CNI and policy engines differ, and a wrong one
would silently drop ingest. Add ingress rules that admit only your agents,
ingress controller/Istio and operators if your cluster supports it.

### PVC permissions

If the pod starts but `serve` loops with an unable-to-open-database error,
check that the volume is group-writable by gid 65532 (or that the StorageClass
honors `fsGroup`). A hostPath/local provisioner that creates root-owned `0755`
directories will not work. Prefer a CSI class that supports `fsGroup`, or
pre-create the volume with the right ownership before the first start. Do not
work around it with `runAsUser: 0`.

## Troubleshooting

- **401 on `/api/...`** — read auth is on and the Bearer token is missing or
  wrong. Mint one with `kubectl exec` as above. `/healthz` is the exception and
  must stay token-free for probes.
- **403 `host not allowed` / `origin not allowed`** — set
  `OTTERSCOPE_ALLOWED_ORIGINS` to the browser-facing origin and restart.
- **404 for `/api/...` or the UI** — a base path is configured but the request
  is outside it, or the Image was built for a different base path. Requests
  outside the prefix are 404 by design, never another app's SPA.
- **Backup Job Pending** — the required pod affinity waits for the server pod,
  or the RWO volume is attached elsewhere. Check
  `kubectl -n otterscope describe pod -l app.kubernetes.io/component=backup`.
- **Backup Job fails with "file exists"** — a retry reused an output path;
  the Job uses the pod name so this indicates a hand-run command, not the
  CronJob. Write to a new file name.
- **CronJob still backing up old data** — the CronJob image is pinned
  separately from the Deployment if you edited only one. Keep them equal.

## Scale-out alternative

There is no supported way to run two writers against one database. For more
ingest capacity, run additional independent Otterscope instances, each with its
own namespace/PVC/database, and point different agents or projects at
different Services. That gives separate databases and separate query surfaces;
it is not a clustered deployment and does not aggregate runs across instances.
