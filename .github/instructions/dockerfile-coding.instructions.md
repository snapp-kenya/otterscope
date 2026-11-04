---
applyTo: "**/dockerfile,**/*.dockerfile"
---

# dockerfile Coding Standards

## Core Principles
1. lowercase everything: commands, variables, labels, stage names
2. no comments, dockerfiles are self-documenting
3. minimal layers, maximum clarity
4. multi-stage builds for images

## Syntax Style
1. commands in lowercase: `from`, `run`, `copy`, `workdir`, `env`, `arg`,
   `expose`, `cmd`, `entrypoint`, etc.
2. variables in lowercase: `arg version=1.0`, etc. Except where uppercase is
   required by the software being installed (e.g. NODE_ENV=production is
   acceptable, as are the typical bash conventions of uppercase environment
   variables, but anything you create should be lowercase).
3. stage names in lowercase: `from ghcr.io/oven-sh/bun:1.1.38-alpine as builder`
4. labels in lowercase: `label maintainer="team@ops.site"`

## Structure
1. group related `run` commands with `&&` and `\` for continuation
2. order instructions from least to most frequently changing (cache
   optimisation)
3. copy the dependency manifests and their lock files in their own layer, before
   the source, so a source change does not re-resolve dependencies
4. use `.dockerignore` to exclude unnecessary files

## Base Images
- fully qualified and patch-pinned, always: registry host, namespace, name, and
  a tag that carries the exact patch version, for example
  `docker.io/library/postgres:16.4-alpine3.20` or
  `ghcr.io/oven-sh/bun:1.1.38-alpine`. There is no acceptable variation: no bare
  names, no floating minor or major tags, and no `latest`
- the patch number is a decision, not a detail: a rebuild must produce the same
  base image, and an upgrade must be a visible edit to the file
- prefer alpine or distroless images, or arch-linux based images if we cannot
  work with alpine or distroless

## Image Tags
1. git is the versioning system. An image this repository builds is tagged with
   the commit sha it was built from, and with nothing else that pretends to be a
   version
2. no semantic versions, no major or minor numbers, no `v1`, no `latest`, no
   `stable`, no date stamps and no build numbers. A second tag that means the
   same thing as a commit is a second truth, and the two drift
3. a mutable tag is never added beside the commit: one image, one tag, one
   commit
4. images are pushed to the registry inside the cluster. Nothing is published to
   a public registry
5. the base image an image is built from is a different matter: it is a third
   party's artefact, so it is fully qualified and patch-pinned as above, because
   its version is not ours to control through git

## Build Arguments & Environment
- `arg` for build-time variables
- `env` for runtime variables
- One env or arg per line - do not combine multiple in a single instruction

## Layers
- combine commands to reduce layers:
```dockerfile
run apk add --no-cache curl && \
    rm -rf /var/cache/apk/*
```
- a dependency layer is copied from the manifest, installed, then invalidated
  only when the manifest or its lock file changes; source is copied after it
- clean up in the same layer that creates files

## Networking
- When we're building our own code, always use port 80
- When packaging third-party software that defaults to another port, use that
  port (e.g., Postgres uses 5432, Redis uses 6379, etc.)

## Multi-stage Builds
e.g:

```dockerfile
from ghcr.io/oven-sh/bun:1.1.38-alpine as build
workdir /app
copy package.json bun.lock ./
run bun install --frozen-lockfile
copy . .
run bun test
run bun build src/main.ts --target bun --outdir dist

from ghcr.io/oven-sh/bun:1.1.38-alpine as runtime
workdir /app
copy package.json bun.lock ./
run bun install --frozen-lockfile --production
copy --from=build /app/dist ./dist
user 1000
expose 80
cmd ["bun", "dist/main.js"]
```

The manifests are copied and installed before the source, so editing a source
file does not reinstall anything. The runtime stage installs production
dependencies only, and copies the built output rather than the source tree.

Where a component genuinely has no dependencies there is no manifest to copy and
no install step; the section is omitted rather than written so that it changes
nothing.

## Security
1. run as non-root user when possible
2. avoid secrets in build args or env
3. use `copy` over `add` unless extracting archives
4. remove unnecessary packages after installation and purge caches, all in a
   single layer

## Final Instructions
1. `expose` documents ports but doesn't publish them
2. Understand the difference between `cmd` and `entrypoint` and use
   appropriately
3. Always use exec form for `cmd`: `cmd ["bun", "run", "server.ts"]`
4. `healthcheck` for container health monitoring

## Avoid
1. mutable tags: `latest`, `stable`, a branch name, a date stamp, or any tag
   that is not the commit sha
2. running as root unless necessary
3. `add` when `copy` suffices
4. multiple `run` commands that could be combined
5. uppercase anything unless absolutely necessary
