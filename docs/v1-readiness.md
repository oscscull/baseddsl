# V1 readiness decision

Assessment date: 2026-10-09. The source version is **0.1.12**, an evaluation
candidate. **NO-GO for public v1 publication until the owner reviews the final
candidate and the conditions below are satisfied.** This review does not merge
PRs, choose a release date, tag/publish artifacts or post an announcement.
Independent adopters and recruited prelaunch volunteers are not prerequisites.

## Promised surfaces and evidence

The [support policy](support-policy.md) defines the v1 compatibility commitment.
The final candidate must retain all of this evidence on its exact commit; a green
ancestor or PR merge ref is not proof of a later main/tag commit.

| Surface | Contract and required evidence |
|---|---|
| BSL/scopes/typed values | [Reference](reference.md), [scope adversarial matrix](scope-contracts.md), parser/sema/runtime tests in workspace; modeled and raw authorization boundaries stay explicit |
| Generated Rust | [Consumer contracts](generated-consumer-contracts.md), fresh external typed consumers on SQLite/MariaDB/PostgreSQL; Rust 1.94 source consumer; matching compiler/runtime regeneration and DTO/schema-edit proof |
| Runtime/transactions | Runtime seam and SQLite tests, both live suites, cancellation/rollback and adopted-pool contracts; [recipes](recipes.md) keep host orchestration explicit |
| HTTP/guards/idempotency | [Deployment](standalone-deployment.md), [callbacks](standalone-guards.md), [durability](standalone-idempotency.md); image, standalone onboarding, TypeScript guards and live store checks; listener access remains restricted behind a trusted edge |
| Migration history | [Recovery and verification](migration-recovery.md), stored snapshots/structural verification, explicit apply/status and failure-recovery suites on each supported database; generated history remains immutable |
| Native/editor distribution | [Installation matrix](installation.md), five native extracted-tool smokes, actual linked SQLite version probe, Rust-floor source consumer and isolated VS Code diagnostics/hover/completion/rename; exact commit/version/checksum collection |
| Existing database adoption | [Import](import-existing-database.md): hand-SQL schemas, metadata-only accounts, fresh typed reads without migrate apply, original schema/data preserved; unsupported facets block with retained native facts |
| First use and edit | [Onboarding gate](onboarding-gate.md) replays embedded and standalone starts from blank projects, production IDs, typed/HTTP create/read, subsequent field rename, regeneration, reviewed migration and retained row; no maintainer repair or adopter quota |

Required ordinary CI jobs are `onboarding`, `initializer`, `consumer-build`,
`cargo-generation`, `runtime-benchmark`, `workspace`, `extension`, `image`,
`live-mariadb`, `live-postgres`, `examples`, `database-tls`, `generated-consumers`
and `typescript-guards`. The native workflow also requires all five `native`
jobs, `source-consumer`, `vsix` and `collect`; its tag-only `draft` is intentionally
skipped for a PR. Every triggered check must pass on the current head. CI checks
contracts; it does not prove market validation or certify security.

## Data-safety finding and resolution

The earlier workspace lock selected `libsqlite3-sys` 0.35.0 / SQLite 3.50.2.
SQLite reports a rare WAL-reset corruption race affecting older engines when
multiple connections write/checkpoint concurrently. The runtime's file-backed
WAL pool makes that upstream finding relevant. Read-only import never writes or
checkpoints, but its safety alone does not resolve a runtime storage defect.
[Upstream WAL explanation](https://www.sqlite.org/wal.html),
[SQLite 3.51.3 fix](https://www.sqlite.org/releaselog/3_51_3.html).

This candidate requires `libsqlite3-sys` **0.37.0**, bundling SQLite **3.51.3**,
within SQLx 0.9's supported `>=0.30.1,<0.38.0` binding range. Both the runtime's
SQLite feature and catalog reader require this floor; an external consumer does
not depend solely on the workspace lockfile. The actual linked-engine test rejects
an older version, and every extracted native CLI proves its engine version through
metadata import. The existing read-only/live-WAL tests compare original bytes;
workspace/runtime and fresh consumer gates verify behavior after the update.

The dependency selection uses the upstream fix; these tests do not claim to
reproduce or statistically rule out the rare race. Arbitrary system SQLite and
consumer-supplied older engines are outside this release's bundled-engine support.
Do not roll back to a vulnerable engine for a WAL deployment. See
[upgrade/recovery boundaries](upgrading.md).

## Costs and maintainer scope

The [build-cost gate](consumer-build-cost.md) accepted a pivot to **explicit
isolated generation**: the optional OUT_DIR helper's measured unchanged-build and
dependency tax is not the starter default. This is an accepted default choice,
not a claim that embedded Rust dependencies have no compilation cost.
The [matched runtime measurements](runtime-performance.md) retain meaningful
large-result/write overhead and their host/workload limits; no universal speed
claim is a launch condition.

Versioned [release notes](release-notes.md), [upgrade guide](upgrading.md),
[contribution tiers](../CONTRIBUTING.md), best-effort
[support](support-policy.md) and the enabled [private security route](../SECURITY.md)
are part of the candidate. No support SLA or every-feature commitment is implied.

## Owner review and remaining conditions

- Review/merge the complete verified stack in dependency order. These PRs remain
  unmerged until explicitly authorized; verify the final main commit again.
- Use the verified public tutorials for the reveal demonstration. The owner
  [closed #71](https://github.com/oscscull/baseddsl/issues/71#issuecomment-5928142400)
  as not worth pursuing; that case study is not a release prerequisite. Do not
  publish private source or claim production adoption from a tutorial.
- Choose the release version, update all matching workspace/extension metadata and
  notes, and approve the compatibility/support scope. `0.1.12` is not a v1 tag.
- Run the final commit's ordinary/native gates, review extracted artifacts and
  checksums, and record the exact approved commit/version before tagging.
- Follow the [owner publication/rollback checklist](releasing.md#final-owner-checklist).
  The [reveal preparation issue](https://github.com/oscscull/baseddsl/issues/72) remains
  preparation; posting requires a separate explicit instruction.

Known rejected import representations stay outside the initial matrix:
[signed native defaults](https://github.com/oscscull/baseddsl/issues/122) and
[arbitrary composite FK aliases](https://github.com/oscscull/baseddsl/issues/123).
They are reported/blocking, rather than silently mapped. No additional feature is
required to claim support for the documented initial matrix. Future recurring use,
concrete bug reports and requests are post-launch evidence; silence or stars do not
establish satisfaction.
