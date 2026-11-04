# DigitalOcean Community draft — Otterscope on a Droplet

*Ready to post at https://www.digitalocean.com/community/questions — voice matched to Kalin's prior guide. Paste as-is or edit.*

---

**Title:** Self-hosting observability for your AI agents on a $6 Droplet

I've been running a few small LLM agents lately, and the thing that kept
biting me wasn't the model — it was not being able to *see* what the agent
actually did. Which tool did it loop on five times? Where did that run
quietly cost me a dollar? Did last week's prompt tweak make things worse or
better? The logs never quite tell you.

The hosted tools for this want your prompts on their servers, and the
self-hostable ones (Langfuse and friends) wanted me to stand up Postgres +
ClickHouse + Redis + S3 — four moving parts to log a few thousand LLM calls
a day. That's a lot of Droplet for a side project.

So I ended up building a small open-source one, Otterscope, and self-hosting
it is genuinely a one-liner. Figured I'd write down how I run it on a
DigitalOcean Droplet, because "AI agent observability" sounds heavier than it
actually is here.

## What it does, quickly

You point your agent's OpenTelemetry exporter at Otterscope and you get an
agent-*run*-first view: every trace becomes a run, with its steps, tool
loops, tokens, and cost. Click a run and you see the actual messages in and
out of each model call. There's a cost table, assertions and LLM-as-judge
evals scored onto your real runs, a compare view for "this week vs last," and
webhook alerts when your error rate or spend crosses a line.

The important part for hosting: it's **one self-contained Rust binary and one
SQLite file.** No database to provision, no cache, no object store. That's the
whole reason it fits on the smallest Droplet.

## The Droplet

A **Basic $6/mo Droplet** (1 vCPU, 1 GB RAM) is plenty unless you're pushing
serious volume — SQLite handles this scale comfortably. Spin one up with
Ubuntu, add your SSH key, done.

There are two ways to run it. Docker is the fastest; the raw binary with
systemd is the leanest.

### Option A — Docker (fastest)

If you ticked the Docker Marketplace image when creating the Droplet, or just
`apt install docker.io`, build the image from the repository — there is no
published image to pull — and run it:

```sh
git clone https://github.com/otterscope/otterscope
cd otterscope
docker build -t otterscope .
docker run -d --restart unless-stopped --read-only \
  --cap-drop ALL --cap-add NET_BIND_SERVICE \
  -p 80:80 \
  -v otterscope:/data \
  otterscope \
  serve -db /data/otterscope.db -read-auth
```

The server has **one listener, always on port 80** — UI, API, `/mcp` and OTLP
ingest all share it. The image is non-root (uid 65532, distroless base); Docker
lets that uid bind 80 without capabilities (its runtime sets
`net.ipv4.ip_unprivileged_port_start=0`), and the `NET_BIND_SERVICE` above keeps
the same command portable to runtimes that keep 1024 privileged. The container
never runs as root. The host side of the mapping is your choice: `-p 80:80` (as
above) or, say, `-p 8080:80` if something else already owns port 80.
`-read-auth` makes the UI and API ask for a token; mint the first token and a
per-agent ingest key against the same volume:

```sh
docker run --rm -v otterscope:/data otterscope \
  token add local -db /data/otterscope.db
docker run --rm -v otterscope:/data otterscope \
  project add my-agent -db /data/otterscope.db
```

The token is shown only now (the database keeps its digest); the project key
stays visible in `project list`. Add `-admin` to `token add` if that token
should also create alerts, assertions or views.

Your browser and your agents both use that one published port; there is no
separate OTLP port any more. Open `http://YOUR_DROPLET_IP` (or
`http://YOUR_DROPLET_IP:8080` if you mapped that instead) once a token is
minted. The `otterscope` volume keeps your data across restarts.

### Option B — the binary + systemd (leanest)

No Docker:

```sh
# build the binary from source (Bun + Rust), then:
cd web && bun install && bun run build
cd ../rust && cargo build --release
sudo mv target/release/otterscope /usr/local/bin/
sudo useradd --system --home /var/lib/otterscope --create-home otterscope
```

Drop a unit at `/etc/systemd/system/otterscope.service`:

```ini
[Unit]
Description=Otterscope
After=network.target

[Service]
User=otterscope
# Port 80 is privileged and the server always binds it; grant the service the
# one capability it needs instead of running as root.
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
ExecStart=/usr/local/bin/otterscope serve -db /var/lib/otterscope/otterscope.db -read-auth
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl enable --now otterscope
sudo -u otterscope otterscope token add local -db /var/lib/otterscope/otterscope.db
```

The `token add` command prints the token once (only its digest is stored), so
paste it into the UI prompt or send it as `Authorization: Bearer <token>`. Add
`-admin` to that command for a token that can also create alerts, assertions
and views. With read auth on, agents need a project key too:
`sudo -u otterscope otterscope project add my-agent -db /var/lib/otterscope/otterscope.db`.

### One security note worth reading

The process always binds `0.0.0.0:80`, so "is it reachable?" is purely a
firewall and publishing question. On a Droplet you have two sane choices:

1. **Keep it private (my preference for a personal instance):** publish the
   container only to loopback (`-p 127.0.0.1:80:80`) and reach the UI over an
   SSH tunnel from your laptop: `ssh -L 8080:localhost:80 you@your-droplet`,
   then open `http://localhost:8080`. With the systemd unit, drop a firewall
   rule that only admits your IP (or deny port 80 entirely and use the
   tunnel). Nothing else on the internet can reach it.
2. **Expose it deliberately:** publish port 80, and put a DigitalOcean
   **Cloud Firewall** in front so only your app servers can reach it. Run with
   `-read-auth` and mint a token (above): Otterscope has token auth but no
   user accounts, so a token is the only thing standing between the internet
   and every trace you have. If you serve it on a hostname, also list that
   origin in `OTTERSCOPE_ALLOWED_ORIGINS` or the API will refuse the browser.

## Point an agent at it

Whatever framework you use — Pydantic AI, the OpenAI Agents SDK, LangGraph,
the Vercel AI SDK — it's the standard OpenTelemetry variable:

```sh
# base URL only: OTLP/HTTP appends /v1/traces itself.
# Use :8080 instead if that is how you published the container.
export OTEL_EXPORTER_OTLP_ENDPOINT=http://YOUR_DROPLET_IP
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <project-ingest-key>"
```

The header is required when `-read-auth` is on (the `project add` command above
prints the key). Run your agent, refresh the UI, and the run shows up in a
couple of seconds.
No SDK to install, no code changes if you're already emitting OTel.

Want to see it populated before wiring a real agent? `otterscope sample`
seeds a batch of realistic demo runs.

## What it adds up to

- **$6/mo** Droplet, and honestly a $4 one would do for light use.
- **Zero** managed-database cost — it's a file on the Droplet's disk. Snapshot
  the Droplet, or run `otterscope backup -o snapshot.db` for a consistent
  copy (a plain `scp` of a live database can miss the WAL).
- Unlimited retention by default (it's your disk); there's a `-retention` flag
  if you'd rather prune.

That's the pitch: the observability stack for your agents is a single process
sitting next to them, costing about as much as a couple of coffees a month.

## Try it

Repo and docs are here: **https://github.com/otterscope/otterscope**
(Apache-2.0). There are per-framework setup guides under `docs/frameworks/`,
and the container image is built from the repository — `docker build -t
otterscope .` — nothing is published to pull.

If you self-host it on a Droplet and hit a rough edge — especially a framework
whose traces don't map cleanly yet — open a PR with the payload. Raw payloads
are retained, so fixes apply retroactively to data you've already collected.
