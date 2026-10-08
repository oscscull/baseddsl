# Contributing

Start with the [principles](spec/principles.md), [support boundary](docs/support-policy.md)
and the issue's acceptance criteria. Keep changes within that issue; discuss new
user-facing behavior separately. Prefer existing patterns and visual behavior.

Every file, function and type must have one plainly stated reason to change.
Separate parsing, checking, lowering, driver I/O, publication and presentation.
Use small focused functions, early returns and functional transformations where
appropriate. SRP is a review requirement, not a line-count target.

## Choose verification for the changed contract

Run focused tests while editing, then the required workspace gate:

```sh
make ci-workspace
```

This runs formatting, the async-free compiler boundary, the reduced-driver graph,
Clippy, workspace tests and documentation tests. It deliberately excludes server
infrastructure; a green fast gate does not prove a server change.

| Change | Focused proof | Required broader evidence |
|---|---|---|
| Lexer/parser/sema/codegen | `cargo test -p based-parser` / `-p based-sema` / `-p based-codegen`, choosing the changed crate | `make ci-workspace`; relevant generated-client behavior must also compile/run |
| Generated Rust/public embedded API | `make ci-generated-consumer-sqlite`; select the corresponding server consumer for server-specific behavior | `make ci-workspace`; full-feature Clippy; existing live/example tier for driver-dependent behavior |
| CLI/generation/import | Focused `based-cli` integration tests; `make ci-onboarding` for first-use behavior | `make ci-workspace`; native/source-consumer checks when distribution paths change |
| SQL/migrations/transactions/catalog readers | Existing `make ci-live-postgres` / `make ci-live-mariadb` against disposable provided fixtures; SQLite runs in workspace | `make ci-workspace`; `cargo clippy --workspace --all-features -- -D warnings`; the affected example/consumer; `make ci-database-tls` for connection/TLS changes |
| HTTP/guards/idempotency | Relevant runtime/CLI integration tests; `make ci-standalone-tutorial` / `make ci-standalone-store` when that contract changes | `make ci-workspace`; existing image/TLS/live tiers as applicable |
| Editor/distribution | `make ci-extension`; existing release dry run on native hosts | Workspace CI plus all triggered native packaging, extracted smoke, Rust-floor source consumer and VS Code runtime checks |
| Build/runtime cost | Existing `make ci-consumer-build`, `make ci-cargo-generation` or `make ci-runtime-benchmark` | Preserve recorded boundaries and parity; timings are evidence, not arbitrary CI thresholds |

The [Makefile](Makefile) documents prerequisites and server URL arguments. Prefer
throwaway databases: live gates create fixtures and some existing runtime suites
reset their provided test database. Never point a live gate at application data.
Do not run gates that rebuild `target/debug/based` with different feature sets
concurrently; compile locks do not protect a running subprocess from another
command replacing that binary.

`make ci-workspace-full` covers server-feature compilation/tests without launching
containers per test. Strict full-feature workspace Clippy is
`cargo clippy --workspace --all-features -- -D warnings`; do not describe that as
an all-target check. Add focused all-target Clippy for changed crates where useful.
The [CI workflow](.github/workflows/ci.yml) and conditional
[release workflow](.github/workflows/release.yml) are the required tier inventory.
Before considering a PR ready, every triggered check must pass on its latest head,
including both onboarding modes and native jobs when triggered. A prior commit's
green run is insufficient. Fix failures; do not disable a gate to obtain a green PR.

Tests should prove observable contracts or meaningful regressions. Reuse existing
fixtures/tiers instead of duplicating service matrices. Keep application code out
of pure compiler crates. Generation must honor artifact ownership; applied
migration history is immutable. See [artifact ownership](docs/generated-artifacts.md)
and [migration safety](docs/migration-recovery.md).

## Send a reviewable change or bug

State the trigger, expected behavior, resulting behavior and verification. Include
a minimal BSL/query and safe reproduction using fabricated data. Record the tool
version/commit and relevant environment; readers should not need compiler-internal
knowledge. Remove URLs with credentials, tokens, private rows and private source.
Ordinary bugs use [GitHub issues](https://github.com/oscscull/baseddsl/issues/new/choose).
Security-sensitive reports use the [private route](SECURITY.md).

Maintainer review and support are best effort; there is no response-time SLA or
promise to implement every request. The [support policy](docs/support-policy.md)
defines supported surfaces. A contribution does not change those boundaries or
license terms. Release publication and merging remain owner decisions.
