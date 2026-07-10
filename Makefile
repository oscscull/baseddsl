# Portable CI targets.
# Every target here is a plain `cargo`/`based`/`npm` invocation runnable ANYWHERE: a laptop,
# GitLab, Buildkite, or the GitHub Actions example in .github/workflows/ci.yml (a thin wrapper
# that provisions service containers and calls these). The DB-backed targets read their server
# URL from a variable/env so they connect to a *provided* DB (a CI service container) — the
# same `DATABASE_URL` convention the quickstarts use — instead of spinning their own.
#
#   make ci-workspace      # fmt + clippy + test (no infra)
#   make ci-extension      # build + package the VS Code extension (needs node/npm)
#   make ci-image          # build the `based serve` image + smoke-boot it (needs docker)
#   make dev-db-up         # throwaway mariadb:11.4 + postgres:16 for local runs
#   make ci-live-mariadb   # migrate-apply + live MariaDB suite against $(MARIADB_URL)
#   make ci-live-postgres  # live Postgres suite against $(POSTGRES_URL)
#   make ci-examples       # build + run the three quickstart scenarios
#   make dev-db-down       # stop the throwaway servers
#
# `make ci` runs the infra-free gates (workspace + extension). The DB targets are separate
# because they need a server; `make ci-live` / `ci-examples` run them once one is up.
#
# Two-tier commit gate (one command per tier, so verifying a change never takes several steps):
#   make check-fast        # iterate: fmt + clippy + full workspace tests. No DB, no examples.
#   make check             # pre-commit for execution-touching changes: check-fast, then fresh
#                          # throwaway DBs + both live suites + all three example scenarios.
# `check` manages its own throwaway DBs (fresh via dev-db-up) and leaves them running for fast
# re-runs; `make dev-db-down` cleans up. Front-end-only changes may gate on check-fast alone.

CARGO ?= cargo
NPM   ?= npm
ROOT  := $(dir $(realpath $(firstword $(MAKEFILE_LIST))))
BASED := $(ROOT)target/debug/based
IMAGE ?= based-serve:ci

# Server URLs. Defaults match `make dev-db-up`'s throwaway containers; override to point a CI
# service container (or any server) at these targets, e.g.
#   make ci-live-postgres POSTGRES_URL=postgres://postgres:pw@localhost:5432/based_test
MARIADB_URL  ?= mysql://root:based_test_pw@127.0.0.1:13306/based_test
POSTGRES_URL ?= postgres://postgres:based_test_pw@127.0.0.1:15432/based_test
SQLITE_DB    ?= quickstart.db

.PHONY: ci check check-fast ci-workspace ci-extension ci-image ci-live ci-live-mariadb \
        ci-live-postgres ci-examples ci-example-sqlite ci-example-mariadb ci-example-postgres \
        based-cli dev-db-up dev-db-down

## Infra-free gate: everything that needs no DB. What `make ci` runs.
ci: ci-workspace ci-extension

## Fast iteration gate: format, lint, full workspace tests. No DB, no examples, no extension.
check-fast: ci-workspace

## Full pre-commit gate: check-fast, then everything DB-backed against fresh throwaway servers
## (started here; left running for fast re-runs — `make dev-db-down` cleans up). Also refreshes
## target/debug/based-lsp — the VS Code extension launches it via a PATH symlink pointing there,
## so a stale binary means the editor silently runs old LSP code.
check: check-fast dev-db-up
	$(CARGO) build -p based-lsp
	$(ROOT)ci/wait-for-db.sh "$(MARIADB_URL)"
	$(ROOT)ci/wait-for-db.sh "$(POSTGRES_URL)"
	$(MAKE) ci-live
	$(MAKE) ci-examples
	@echo "check: all gates green"

## Workspace gate: format, lint, and the full test suite (mirrors the commit gate).
ci-workspace:
	$(CARGO) fmt --check
	$(CARGO) clippy --workspace --all-features -- -D warnings
	$(CARGO) test --workspace --all-features

## Build + package the VS Code extension (.vsix). `npm ci` is the reproducible install.
ci-extension:
	cd editors/vscode && $(NPM) ci && $(NPM) run compile && $(NPM) run package

## Build the `based serve` container image, then smoke-boot it against
## bundled SQLite (no external DB) to prove the packaged image actually serves — the deploy
## artifact never rots. Needs Docker; the smoke script curls /healthz + /readyz.
ci-image:
	docker build -f docker/Dockerfile -t $(IMAGE) .
	$(ROOT)ci/smoke-image.sh $(IMAGE)

## Build the `based` CLI once; the example targets shell out to it.
based-cli:
	$(CARGO) build -p based-cli

## All live-DB proof (both dialects). Assumes both servers are up (see dev-db-up).
ci-live: ci-live-mariadb ci-live-postgres

## Live MariaDB: the integration suite + `based migrate apply` (E4), both against a PROVIDED
## server. `TEST_MARIADB_URL` makes the harness connect there instead of spinning a container
## (support/docker_mariadb.rs); `--test-threads=1` keeps the shared DB's per-test resets serial.
ci-live-mariadb:
	TEST_MARIADB_URL="$(MARIADB_URL)" $(CARGO) test -p based-runtime --features docker-tests \
	  --test mariadb_integration --test migrate_apply_mariadb -- --test-threads=1 --nocapture

## Live Postgres: the integration suite against a PROVIDED server (`TEST_POSTGRES_URL`).
ci-live-postgres:
	TEST_POSTGRES_URL="$(POSTGRES_URL)" $(CARGO) test -p based-runtime --features docker-tests \
	  --test postgres_integration -- --test-threads=1 --nocapture

## The three example scenarios: `based migrate apply` then `cargo run`, each end-to-end green.
## Each expects an empty DB (a fresh CI service container / throwaway); the SQLite one resets
## its own file. This is the example half of DoD #4 (the copyable quickstarts never rot).
ci-examples: ci-example-sqlite ci-example-mariadb ci-example-postgres

ci-example-sqlite: based-cli
	cd examples/sqlite-quickstart && rm -f "$(SQLITE_DB)" && \
	  DATABASE_URL="$(SQLITE_DB)" $(BASED) migrate apply --database-url "$(SQLITE_DB)" && \
	  DATABASE_URL="$(SQLITE_DB)" $(CARGO) run

ci-example-mariadb: based-cli
	$(ROOT)ci/wait-for-db.sh "$(MARIADB_URL)"
	cd examples/mariadb-quickstart && \
	  DATABASE_URL="$(MARIADB_URL)" $(BASED) migrate apply --database-url "$(MARIADB_URL)" && \
	  DATABASE_URL="$(MARIADB_URL)" $(CARGO) run

ci-example-postgres: based-cli
	$(ROOT)ci/wait-for-db.sh "$(POSTGRES_URL)"
	cd examples/postgres-quickstart && \
	  DATABASE_URL="$(POSTGRES_URL)" $(BASED) migrate apply --database-url "$(POSTGRES_URL)" && \
	  DATABASE_URL="$(POSTGRES_URL)" $(CARGO) run

## Local convenience: throwaway mariadb:11.4 + postgres:16 matching the default URLs above.
## CI provisions these as service containers instead (see .github/workflows/ci.yml).
dev-db-up:
	-docker rm -f based-ci-maria based-ci-pg 2>/dev/null
	docker run --rm -d --name based-ci-maria -p 13306:3306 \
	  -e MARIADB_ROOT_PASSWORD=based_test_pw -e MARIADB_DATABASE=based_test mariadb:11.4
	docker run --rm -d --name based-ci-pg -p 15432:5432 \
	  -e POSTGRES_PASSWORD=based_test_pw -e POSTGRES_DB=based_test postgres:16

dev-db-down:
	-docker rm -f based-ci-maria based-ci-pg 2>/dev/null
