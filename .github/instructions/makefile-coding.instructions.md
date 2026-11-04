---
applyTo: "**/makefile"
---

# makefile Coding Standards

## Purpose
- In this project, makefiles are workflow orchestrators, not build systems
- actual software builds happen inside dockerfiles
- make provides convenient shortcuts for docker commands, which is better than
  writing a bunch of shell-scripts

## Core Principles
1. lowercase everything: variables, targets, filenames, except where required by
   the software being invoked
2. no comments unless absolutely necessary
3. minimal abstraction, each target should be obvious
4. `.phony` for all targets (nothing produces files)

## Syntax Style
1. variables in lowercase: `image_name := ger`
2. targets in lowercase: `build:`, `run:`, `push:`
3. use `:=` for immediate assignment, `=` only when lazy evaluation needed
4. indent recipes with tabs (required by make)

## Variables
```makefile
image_name := ger
registry := localhost:5500/ger
tag := latest
port := 80
```

## Common Targets
```makefile
.PHONY: build run stop clean push pull logs shell

build:
	docker build -t $(image_name):$(tag) .

run:
	docker run -d --name $(image_name) -p $(port):80 $(image_name):$(tag)

stop:
	docker stop $(image_name) && docker rm $(image_name)

clean:
	docker rmi $(image_name):$(tag)

push:
	docker push $(registry)/$(image_name):$(tag)

pull:
	docker pull $(registry)/$(image_name):$(tag)

logs:
	docker logs -f $(image_name)

shell:
	docker exec -it $(image_name) /bin/sh
```

## Target Naming
- single-word lowercase: `build`, `run`, `push`
- hyphenated for multi-word: `build-dev`, `run-watch`
- group related targets with prefixes: `db-start`, `db-stop`, `db-migrate`

## Dependencies
- chain targets when order matters: `deploy: build push`
- keep chains short and obvious

## Patterns
- `@` prefix suppresses command echo: `@echo "done"`
- use `$(MAKE)` for recursive make calls
- `||` for fallback commands: `docker stop x || true`

## Environment
- read from environment with defaults: `tag ?= latest`
- pass build args: `--build-arg version=$(version)`

## Avoid
1. uppercase anything
2. complex shell scripting in recipes, use a script file instead
3. file-based targets (everything is .PHONY)
4. tab/space mixing (use tabs, not spaces, for recipe indentation)
5. deeply nested variable references
