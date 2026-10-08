# Upgrade a pinned Based deployment

Upgrade compiler, LSP, extension and runtime together to the reviewed version and
source commit. Keep the application lockfile and schema assets versioned. Start
with the [candidate's versioned notes](release-notes.md), the
[support/compatibility policy](support-policy.md) and the generated diff; `0.x`
compatibility is experimental and does not imply the v1 stability promise.

## Regenerate and verify offline

Back up the consuming project and preserve its applied migration history. Install
matching tools, confirm both `--version` identities, pin the Rust runtime to the
same full published Git revision/tag, and update the consumer's lockfile deliberately.
Regenerate through the existing configured workflow:

```sh
based check
based gen all
based gen all --check
based migrate verify
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Standalone hosts also need matching runtime schema assets and HTTP/error/DTO tests;
replay authenticated context/guard callback and idempotency behavior with the
[standalone tutorial](standalone-tutorial.md). Embedded applications verify their
production ID generator, caller-owned pool/transactions and typed calls. Keep
artifact ownership rules; generation does not silently replace hand-owned imports.
Do not run the optional Cargo helper and explicit generation against the same
output file.

Earlier experimental decimal clients must move to the generated `client::Decimal`
and matching `bigdecimal` dependency as described in the release notes. Any wire,
public API or DTO change needs call-site adaptation and application regression
proof; regenerating successfully does not verify every business use.

## The SQLite engine update

The reviewed workspace moves from bundled SQLite 3.50.2 (`libsqlite3-sys` 0.35.0)
to **3.51.3** (`libsqlite3-sys` 0.37.0), which includes the
[upstream WAL-reset fix](https://www.sqlite.org/releaselog/3_51_3.html).
The runtime's SQLite feature requires the binding floor independently of the
workspace lockfile. Update/rebuild the consuming application's lockfile/runtime;
a new CLI alone does not replace the SQLite engine linked into an old executable.
The release's extracted CLI probe and runtime engine-version test verify the
actual linked version. This update adds no automatic schema migration.

Stop older writer/checkpointer processes before deploying rebuilt binaries against
the same file. Preserve the database with SQLite's supported backup procedure;
a copy of only the main `.db` file while committed changes remain in WAL is not a
complete live backup. Test restore and application reads using fabricated/staging
data before upgrading a production deployment.
[SQLite WAL/backup guidance](https://www.sqlite.org/wal.html).
The upstream fix prevents the identified race; it does not repair pre-existing
corruption or replace database-specific recovery assessment.

## Review and apply database changes separately

Client/tool upgrades do not apply migrations. Inspect `based migrate status` against
the intended configured database, review generated structural changes and rendered
SQL, then apply only the selected approved history. Use
[migration recovery](migration-recovery.md) for failed apply and partial MariaDB DDL;
do not rewrite an already-applied `.mig`/snapshot to make verification pass.
One-shot imported schemas remain owned by the existing migration system; an upgrade
is not permission to generate a baseline or take over that history.

Binary rollback, schema rollback and data restore are different operations.
Reinstalling an older compiler/runtime does not reverse an applied migration or
restore rows. Check whether the old runtime can read the new schema; use a reviewed
inverse migration or tested backup restore when required. Never downgrade a WAL
writer to the vulnerable engine. For a bad published artifact, direct users to a
known safe pinned version and issue a new corrected version/advisory; do not move
published tags or replace binaries silently. Use the
[private security route](../SECURITY.md) for sensitive findings.
