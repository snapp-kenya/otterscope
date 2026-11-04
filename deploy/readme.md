# Deploying Otterscope on k3d

Otterscope runs on **Kubernetes via k3d on the operator's MacBook Pro**. That is
the only deployment target. There is no registry step, no hosted image, no
archive and no compose file: [`deploy.sh`](k8s/deploy.sh) builds the image from
the current commit, tags it with the short git SHA, imports it into the local
k3d cluster and applies the manifests.

Plain YAML plus `kustomize` (the one built into `kubectl`). There is no Helm
chart, no chart dependency and no templating step. `kubectl kustomize` renders
seven objects; the script only substitutes the image tag.

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
one port. TLS and external hostnames are the cluster's business, not this
Deployment's. [ADR-0007](../docs/adr/0007-one-port-80.md) and
[ADR-0009](../docs/adr/0009-kubernetes-k3d-deployment.md) record those
decisions.

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

- An Apple-silicon Mac (the image is built for the local Docker architecture).
- Docker Desktop, running.
- `k3d` (`brew install k3d`) and `kubectl` (`brew install kubectl`).
- `git`, on branch with a clean working tree.
- No registry account, no image push, no Helm. The image lives in Docker's
  local store and in the k3d cluster's containerd store.

## Deploy

From the repository root:

```sh
deploy/k8s/deploy.sh
```

The script refuses to run on a dirty tree (pass `--allow-dirty` only when you
deliberately want to deploy an uncommitted tree; the image is still tagged with
`HEAD`'s SHA). `--dry-run` prints the plan without building, importing or
applying anything.

```sh
deploy/k8s/deploy.sh --dry-run
```

## What `deploy.sh` does

1. Resolves `sha=$(git rev-parse --short HEAD)` and checks the tree is clean.
2. Builds the image from the repository root:
   `docker build --build-arg version="$sha" -t "otterscope:$sha" .`. The build
   arg reaches the binary: the dockerfile exports `OTTERSCOPE_VERSION`, and
   `build.rs` compiles it into `otterscope::VERSION`, so `otterscope version`,
   `/healthz` and `/metrics` all report the SHA.
3. Ensures the k3d cluster exists. It uses `K3D_CLUSTER` (default `otterscope`)
   and creates the cluster with one server, zero agents and `--wait` if it is
   missing. It then selects the `k3d-<cluster>` kubectl context so the commands
   it prints target that cluster.
4. Imports the local image into that cluster with
   `k3d image import "otterscope:$sha" -c "$K3D_CLUSTER"`. Nothing is pushed.
5. Renders `kubectl kustomize deploy/k8s`, fails if the `otterscope:dev`
   placeholder is absent, substitutes `otterscope:$sha`, and applies the result
   with `kubectl apply -f -`.
6. Waits for `kubectl -n otterscope rollout status deployment/otterscope` with
   `ROLLOUT_TIMEOUT` (default `300s`).
7. Prints the port-forward and token-bootstrap commands.

Manual equivalents, if you need to run a step yourself:

```sh
k3d cluster list
k3d cluster create otterscope --servers 1 --agents 0 --wait   # only if missing
docker image ls otterscope
kubectl kustomize deploy/k8s | grep 'image:'
```

`kubectl apply -k deploy/k8s` on its own is **not** the install: it would leave
the `otterscope:dev` placeholder tag, which no cluster can pull. Go through the
script.

## Reach the UI and API

The Service is ClusterIP and port 80. Port-forward it:

```sh
kubectl -n otterscope port-forward svc/otterscope 8080:80
# in another shell:
curl -fsS http://127.0.0.1:8080/healthz
# {"status":"ok","version":"<short-sha>"}
```

`/healthz` is public even with read auth on. It is also what the startup,
readiness and liveness probes use. The `version` field is the SHA printed by
`deploy.sh`; check the binary directly too:

```sh
kubectl -n otterscope exec deploy/otterscope -- otterscope version
```

Both must print the same value.

## First token and first ingest key

`OTTERSCOPE_READ_AUTH=true` is the default in `configmap.yaml` and is the
correct posture for anything beyond loopback. The UI and `/api` need a Bearer
token; OTLP ingest needs a project ingest key. Both are minted by the CLI
against the same SQLite database.

Run the CLI **inside the running server pod** with `kubectl exec`. That is
deliberate: it is the same process namespace, the same node and the same
database file as the writer, so it does not create a second pod competing for
the `ReadWriteOnce` volume. The `OTTERSCOPE_DB` value comes from the ConfigMap,
so `-db` is not needed.

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
list` never print secrets again. Store them in your password manager. To capture
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
Job pod would be a second pod on the single-writer volume.

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

With read auth off, keyless ingest falls into the `default` project. Do not
turn read auth off: it also opens the whole read/write API.

## Probes, resources and the pod contract

The Deployment is a hardened single pod:

- startup probe: `GET /healthz` every 2s, up to 150 failures (300 seconds for a
  large database to migrate);
- readiness probe: `GET /healthz` every 5s;
- liveness probe: `GET /healthz` every 10s;
- requests `100m` CPU / `256Mi` memory, limits `1` CPU / `1Gi`;
- `strategy: Recreate`, `replicas: 1`, `minReadySeconds: 5` — a rollout is a
  short outage, never two writers on the volume.

The backup CronJob requests `50m` CPU / `64Mi` memory and limits `500m` /
`512Mi`. It uses the same image as the server (the script substitutes both).

## Storage

One `ReadWriteOnce` PVC, `otterscope-data`, 10Gi, mounted at `/data`. k3d's
default `local-path` StorageClass provisions it on the node; `fsGroup: 65532`
and `fsGroupChangePolicy: OnRootMismatch` make it writable by the image's
uid/gid 65532. The database, WAL and shared-memory files live at `/data`, which
is the only writable path in the container (`TMPDIR` and `SQLITE_TMPDIR` point
there too). The PVC survives pod deletes and rollouts; it is deleted with the
cluster (see "Teardown").

If the pod starts but `serve` loops with an unable-to-open-database error,
the StorageClass is not honoring `fsGroup`. Prefer a CSI class that does. Do
not work around it with `runAsUser: 0`.

## Backups

`cronjob.yaml` runs `otterscope backup` daily at 03:17 UTC. It writes a
consistent snapshot to the PVC:

```text
/data/backups/otterscope-<pod-name>.db
```

The pod name is unique per attempt, so a retried Job does not collide with an
existing file. The backup subcommand creates the `backups` directory itself;
the image is distroless and has no shell or `mkdir`. The Job runs on the same
node as the server (`podAffinity`) because the PVC is `ReadWriteOnce`; with one
k3d server that is always true, and the affinity also keeps the intent explicit
if the cluster grows.

Run one now and inspect it:

```sh
kubectl -n otterscope create job --from=cronjob/otterscope-backup \
  otterscope-backup-manual
kubectl -n otterscope wait --for=condition=Complete \
  job/otterscope-backup-manual --timeout=10m
kubectl -n otterscope logs job/otterscope-backup-manual
# backed up /data/otterscope.db to /data/backups/otterscope-....db
```

The image has no `ls`, so inspect or copy the directory from a temporary pod
that mounts the same PVC:

```sh
kubectl -n otterscope run pvc-inspect --rm -it --restart=Never \
  --image=busybox:1.37 \
  --overrides='{"spec":{"automountServiceAccountToken":false,"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"fsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"pvc-inspect","image":"busybox:1.37","command":["sleep","600"],"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"volumeMounts":[{"name":"data","mountPath":"/data"}]}],"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"otterscope-data"}}]}}'
# in another shell:
kubectl -n otterscope exec pvc-inspect -- ls -ln /data/backups
kubectl -n otterscope cp pvc-inspect:/data/backups/otterscope-....db ./snapshot.db
```

Two operational facts the manifest cannot hide:

- **Snapshots accumulate on the same volume.** The CronJob does not prune
  files, and the distroless image has no `rm`. Copy snapshots to the Mac on
  your own schedule, then remove old ones with the temporary pod. Monitor free
  space; `OTTERSCOPE_RETENTION` bounds the database's own growth, not the
  backup directory's.
- **The backup process opens the database.** It must use the same image version
  as the server; a newer backup image can apply a pending migration that the
  running server does not understand. `deploy.sh` substitutes the same SHA into
  both the Deployment and the CronJob.

### Pre-upgrade snapshot and restore

> **Schema baseline.** This build starts from a new SQLite baseline
> (`0001_initial.sql`, [ADR-0008](../docs/adr/0008-sqlite-schema-redesign.md)).
> A database created by an older Otterscope build is **refused**, not migrated
> in place: the pod exits with an error that says to start a new database.
> There is no in-place upgrade path from a pre-baseline database; keep its
> snapshot for the record, start a fresh volume and re-ingest, or stay on the
> older image. Future schema changes after the baseline are still new numbered
> migrations, and those do keep the snapshot-before-migrate behaviour described
> here.

The server snapshots an existing database next to itself before applying
pending migrations (`/data/otterscope.db.pre-migrate-<ns>.bak`, only the newest
is kept). Take an explicit snapshot with a name you can find:

```sh
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
kubectl -n otterscope exec deploy/otterscope -- \
  otterscope backup -o "/data/backups/pre-upgrade-${STAMP}.db"
```

A database migrated by a newer version cannot be opened by an older one, so a
rollback is restore-then-rollback, never just an older image on the migrated
file. To restore: scale the Deployment to 0, replace `/data/otterscope.db` with
the chosen snapshot from the temporary pod (and remove the stale
`otterscope.db-wal`/`otterscope.db-shm`), then scale back to 1 and confirm
`/healthz` and a known run. Do this with the writer stopped; never swap the
file under a running server.

## Upgrade

The image tag is the commit SHA, so an upgrade is a checkout plus a re-run:

```sh
git checkout main
git pull
# optional, and recommended: take a snapshot first (above)
deploy/k8s/deploy.sh
```

The script builds the new SHA, imports it, re-renders the manifests with the new
tag and waits for the rollout. `strategy: Recreate` gives a short outage: the
old pod is stopped before the new one starts, so there is never a second writer
on the volume. The PVC and its data stay in place across the rollout.

Until the new version is confirmed, keep the previous SHA in the reflog and its
snapshot. Readiness gates on `/healthz`; the startup probe allows up to 300
seconds for a large database to migrate. If a post-baseline migration fails, the
pod crash-loops and the pre-migration snapshot is the recovery point. A
pre-baseline database is refused before any snapshot is written; keep the
original file.

## Rollback

There is no tag to move and nothing to pull. To roll back, deploy an earlier
commit:

```sh
git checkout <earlier-sha>          # or: git switch -c rollback <earlier-sha>
deploy/k8s/deploy.sh
```

This rebuilds (or reuses the cached build of) that commit, tags it with that
SHA, imports it and rolls the Deployment. The database caveat above still
applies: if the newer image applied a migration, restore the pre-upgrade
snapshot before starting the older binary. A pre-baseline database is never
migrated, so it is also never a rollback problem.

The previous images remain in the Docker daemon and in the k3d cluster's
containerd store; there is no cleanup step required.

## Optional path-based Ingress (disabled by default)

`ingress.yaml` is not in `kustomization.yaml`. It demonstrates serving the whole
app under `/otterscope` with one path rule and no rewrite annotation: the app
strips the prefix itself. `deploy.sh` always builds the root-path image, so a
prefixed deployment is a manual variant. Enabling it needs all of these,
because a base path is a build-time + runtime setting, not just an Ingress
rewrite:

1. **Build the image with the same prefix.** An image built without it is
   root-only, and the React bundle bakes its API and router base into the
   JavaScript at build time:

   ```sh
   docker build --build-arg version="$(git rev-parse --short HEAD)" \
     --build-arg OTTERSCOPE_BASE_PATH=/otterscope \
     -t "otterscope:$(git rev-parse --short HEAD)" .
   k3d image import "otterscope:$(git rev-parse --short HEAD)" -c otterscope
   ```

   A root-built image under this Ingress serves the HTML but its JS calls
   `/api`, which the Ingress does not route. If the baked base and
   `OTTERSCOPE_BASE_PATH` disagree the server still starts (API, MCP and OTLP
   remain valid) but logs a prominent `WARN` naming the build arg to use, and
   the UI will not work.
2. Set `OTTERSCOPE_BASE_PATH: "/otterscope"` and
   `OTTERSCOPE_ALLOWED_ORIGINS: "https://otterscope.example.com"` in
   `configmap.yaml`.
3. Change all three probe paths in `deployment.yaml` from `/healthz` to
   `/otterscope/healthz`.
4. Uncomment `ingress.yaml` in `kustomization.yaml`, or
   `kubectl apply -f deploy/k8s/ingress.yaml` after editing the host.

Then the UI/API is at `https://otterscope.example.com/otterscope/` and the OTLP
endpoint is `https://otterscope.example.com/otterscope` (the exporter appends
`/v1/traces`). External agents also need to send the project ingest key. A path
rewrite to `/` is wrong for this setup: the app's emitted URLs already carry the
prefix. TLS is the cluster's business: point `tls.secretName` at cert-manager or
your controller, or let a gateway own it.

## Auth, origins and TLS

- `OTTERSCOPE_READ_AUTH: "true"` is the manifest default. Token scopes are read
  and admin; mutating routes need admin.
- If the UI is reached through a hostname, add the external origin to
  `OTTERSCOPE_ALLOWED_ORIGINS` in `configmap.yaml`, then restart the Deployment.
  Without it the browser request is refused with
  `403 {"error":"host not allowed"}` or `{"error":"origin not allowed"}`.
  Loopback and IP literals are accepted without an entry.
- TLS is terminated outside this Deployment. The Service is ClusterIP and
  deliberately not exposed with `NodePort` or `LoadBalancer`; port-forward is
  the local access path.

## Teardown

```sh
k3d cluster delete otterscope
```

That deletes the cluster's containers and, with them, the `local-path` volume
holding `/data`. **The database is gone.** Copy the snapshots you care about to
the Mac first (see "Backups"). The locally built `otterscope:<sha>` images stay
in Docker's image store; `docker image rm otterscope:<sha>` removes one you no
longer need. No remote copy exists anywhere.

## Troubleshooting

- **`deploy: working tree is dirty`** — commit or stash, or pass
  `--allow-dirty` if you really mean to deploy uncommitted code. The image tag
  is still `HEAD`'s SHA, so an `--allow-dirty` image does not match its tag.
- **Image pull error in the pod** — the image was not imported. Check
  `k3d image import "otterscope:$sha" -c otterscope` ran for the SHA in the
  Deployment, then `kubectl -n otterscope describe pod`.
- **401 on `/api/...`** — read auth is on and the Bearer token is missing or
  wrong. Mint one with `kubectl exec` as above. `/healthz` is the exception and
  must stay token-free for probes.
- **403 `host not allowed` / `origin not allowed`** — set
  `OTTERSCOPE_ALLOWED_ORIGINS` to the browser-facing origin and restart.
- **404 for `/api/...` or the UI** — a base path is configured but the request
  is outside it, or the image was built for a different base path. Requests
  outside the prefix are 404 by design, never another app's SPA. A root-built
  image started with a non-empty base path logs a `WARN` naming the build arg
  to rebuild with; the API, MCP and OTLP receiver still work.
- **`/healthz` version is not a SHA** — the image was built by hand without
  `--build-arg version=<sha>`, or you are looking at an older pod. Re-run
  `deploy.sh`; a plain `cargo build` reports the crate version, not a commit.
- **Backup Job Pending** — the required pod affinity waits for the server pod,
  or the RWO volume is attached elsewhere. Check
  `kubectl -n otterscope describe pod -l app.kubernetes.io/component=backup`.
- **Backup Job fails with "file exists"** — a retry reused an output path; the
  Job uses the pod name, so this indicates a hand-run command, not the CronJob.
  Write to a new file name.
- **CronJob still backing up old data** — its image tag is stale. Re-run
  `deploy.sh`; it substitutes the same SHA into the Deployment and the CronJob.

## Security posture

The manifests are compatible with the namespace's `restricted` Pod Security
Standard:

- `runAsNonRoot: true`, `runAsUser`/`runAsGroup`/`fsGroup: 65532`;
- `readOnlyRootFilesystem: true`, with `/data` (the PVC) as the only writable
  path; `TMPDIR` and `SQLITE_TMPDIR` already point at `/data`;
- `allowPrivilegeEscalation: false`, capabilities `drop: [ALL]`;
- `NET_BIND_SERVICE` is the single capability added, only for the server
  container, because uid 65532 binds port 80 and Kubernetes/containerd keeps
  the unprivileged-port boundary at 1024. The backup container does not bind
  and does not get it;
- `seccompProfile: RuntimeDefault` at pod level;
- `automountServiceAccountToken: false` on the ServiceAccount and every pod;
- no RBAC objects; the workload never calls the Kubernetes API.

No NetworkPolicy is included: a wrong one would silently drop ingest. Add
ingress rules that admit only your agents and operators if your cluster supports
them.

## Scale-out alternative

There is no supported way to run two writers against one database. For more
ingest capacity, run additional independent Otterscope instances, each with its
own namespace/PVC/database, and point different agents or projects at different
Services. That gives separate databases and separate query surfaces; it is not
a clustered deployment and does not aggregate runs across instances.
