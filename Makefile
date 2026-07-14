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
#   make ci-examples       # build + run the quickstart scenarios + the helpdesk smoke
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

.PHONY: ci check check-fast ci-workspace ci-coloring ci-extension ci-image ci-live \
        ci-live-mariadb ci-live-postgres ci-live-sqlx ci-examples ci-example-sqlite \
        ci-example-mariadb ci-example-postgres ci-example-helpdesk based-cli dev-db-up dev-db-down

# The front-end crates that must stay async-runtime-free (parse → fmt → sema → codegen →
# facts stay sync + pure; only the runtime and binaries may depend on tokio/sqlx).
FRONTEND_CRATES := based-ast based-parser based-fmt based-sema based-codegen based-facts \
                   based-diagnostics based-manifest

## Infra-free gate: everything that needs no DB. What `make ci` runs.
ci: ci-workspace ci-extension

## Fast iteration gate: format, lint, full workspace tests. No DB, no examples, no extension.
check-fast: ci-workspace

## Full pre-commit gate: check-fast, then everything DB-backed against fresh throwaway servers
## (started here; left running for fast re-runs — `make dev-db-down` cleans up). Also refreshes
## target/debug/based-lsp — the VS Code extension launches it via a PATH symlink pointing there,
## so a stale binary means the editor silently runs old LSP code.
## The servers are re-freshed between the live suites and the example scenarios: the live
## suites reset per test *at start* and leave their last schema/ledger behind, while each
## example expects an empty database — the same isolation CI gets from one service
## container per job (.github/workflows/ci.yml).
check: check-fast dev-db-up
	$(CARGO) build -p based-lsp
	$(ROOT)ci/wait-for-db.sh "$(MARIADB_URL)"
	$(ROOT)ci/wait-for-db.sh "$(POSTGRES_URL)"
	$(MAKE) ci-live
	$(MAKE) dev-db-up
	$(ROOT)ci/wait-for-db.sh "$(MARIADB_URL)"
	$(ROOT)ci/wait-for-db.sh "$(POSTGRES_URL)"
	$(MAKE) ci-examples
	@echo "check: all gates green"

## Workspace gate: format, lint, the coloring boundary, and the full test suite.
ci-workspace: ci-coloring
	$(CARGO) fmt --check
	$(CARGO) clippy --workspace --all-features -- -D warnings
	$(CARGO) test --workspace --all-features

## Coloring boundary: every front-end crate's dependency tree must be free of async
## runtimes and drivers (tokio/sqlx/futures/async-*). Fails loudly on a leak.
ci-coloring:
	@for c in $(FRONTEND_CRATES); do \
	  hits=$$($(CARGO) tree -p $$c -e normal --prefix none --all-features 2>/dev/null \
	    | grep -E '^(tokio|sqlx|futures|async-std|async-trait|smol|mio|hyper|axum)[ -]' || true); \
	  if [ -n "$$hits" ]; then \
	    echo "coloring violation: $$c depends on an async runtime:"; echo "$$hits"; exit 1; \
	  fi; \
	done; echo "ci-coloring: front end is async-free"

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
ci-live: ci-live-mariadb ci-live-postgres ci-live-sqlx

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

## The sqlx codec-fidelity spike: the lowered SQL's values round-trip through sqlx on all
## three dialects (MariaDB via its MySql driver, Postgres, SQLite on a temp file).
ci-live-sqlx:
	TEST_MARIADB_URL="$(MARIADB_URL)" TEST_POSTGRES_URL="$(POSTGRES_URL)" \
	  $(CARGO) test -p based-runtime --features docker-tests \
	  --test sqlx_spike -- --test-threads=1 --nocapture

## The example scenarios: `based migrate apply` then `cargo run`, each end-to-end green.
## Each expects an empty DB (a fresh CI service container / throwaway); the SQLite one resets
## its own file, and the helpdesk smoke resets the shared Postgres itself (so it runs last).
## This is the example half of DoD #4 (the copyable examples never rot).
ci-examples: ci-example-sqlite ci-example-mariadb ci-example-postgres ci-example-helpdesk

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

## The flagship axum service, end to end over real HTTP: reset the database (drop +
## recreate `public`, so a shared throwaway server is fine), migrate, seed, then boot
## the service and drive every route (auth, scoping, guard, idempotency, NDJSON export).
ci-example-helpdesk: based-cli
	$(ROOT)ci/wait-for-db.sh "$(POSTGRES_URL)"
	cd examples/axum-helpdesk && \
	  DATABASE_URL="$(POSTGRES_URL)" $(CARGO) run --bin smoke -- reset && \
	  DATABASE_URL="$(POSTGRES_URL)" $(BASED) migrate apply --database-url "$(POSTGRES_URL)" && \
	  DATABASE_URL="$(POSTGRES_URL)" $(CARGO) run --bin seed && \
	  DATABASE_URL="$(POSTGRES_URL)" $(CARGO) run --bin smoke

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
