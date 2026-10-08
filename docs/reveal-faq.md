# Short reveal answers

**Why not SQLx/Diesel/SeaORM?** They already provide async and typed access. Based
moves modeled policy, projection and callable declarations into one checked contract
and generates the boundary. Choose an established approach when direct SQL control,
broader capabilities, maturity or conversion costs matter more. See the [support boundary](support-policy.md) and
[execution reference](reference.md) before adopting.

**What executes it?** The Based engine and underlying SQLx driver. Embedded Rust
uses an in-process JSON-value bridge; standalone adds HTTP and a trusted auth edge.
Both paths have first-use/edit automation. Host code supplies authenticated context,
external/embedded guards and transaction/business orchestration.

**Databases?** The reviewed bundled SQLite engine (3.51.3+ WAL-reset fix), MariaDB
11.4 and PostgreSQL 16. MariaDB CI does not claim MySQL server support; untested
native facets and platforms stay outside the matrix. See [support](support-policy.md) and [installation](installation.md).

**Build/runtime cost?** See [runtime measurements](runtime-performance.md) and
[build-cost evidence](consumer-build-cost.md). The optional Cargo helper's
unchanged-build tax led to explicit isolated generation by default. Large flat/nested
reads and bulk writes measured 1.85×/1.40×/2.40× median overhead versus matched direct
SQLx on one host; small timing differences overlap noise. No general speed claim.

**License?** AGPL-3.0-only source/runtime/extension, with separate generated-output
terms. See the [source license](../LICENSE) and
[generated-output terms](../LICENSE-GENERATED.md); no alternate commercial
license or paid support promise is offered here.

**Migrations/import?** Planning/generation are offline and apply is explicit. Stored
history is immutable and recovery depends on the database. One-shot import reads
metadata and publishes checked hand-owned models without baselining or migration
takeover; the fresh typed proof reads each original database unchanged.

**Escape hatches/limitations?** Raw SQL binds parameters but you own filters, type/
result correctness, portability and opaque-query edits. Host transactions cover
read-decide-write orchestration; guard approval is not an atomic write invariant.
Unsupported import representations block with native facts retained.

**AI?** An unmeasured hypothesis; there is no productivity/review-burden statistic.
The DSL still requires schema, policy, raw-SQL and migration review.

**Report a bug?** Version, relevant model/query/call, expected/actual result and a
safe reproduction. No compiler-internal knowledge, private database dump, analytics
or account/signup beyond the user's chosen GitHub reporting channel. Security
reports use the [private route](../SECURITY.md); support is best effort with no SLA.
