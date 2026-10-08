# Prepared public reveal

This is an announcement draft for an owner-selected venue, not a published launch.
The owner closed [#71](https://github.com/oscscull/baseddsl/issues/71#issuecomment-5928142400)
as not worth pursuing. This demonstration uses the verified public starter instead.
Before posting, complete the [publication checklist](releasing.md#final-owner-checklist),
verify the actual release installation links on its exact commit, and remove these
preparation notes. A PR dry run is not a shipped release. Posting requires explicit
owner instruction; no announcement or outreach has been sent.

## Announcement draft

**Based: review ownership and typed results in one database contract**

Based puts a modeled ownership rule, result projection and callable in one readable
contract, then generates the typed async Rust boundary. This is part of its runnable
[embedded starter](../crates/based-cli/starter/schema/item.bsl):

```bsl
scope Author (owner: uuid = $ctx.owner)

@scope Author
Item {
  id: Id
  name: text
  owner: uuid
  parent: Item?
  @index owner
}

shape ItemView from Item {
  id
  name
  parent { id, name }
}
```

```bsl
query items() -> ItemView[] scoped Author {
  list Item order (id);
}
```

With the engine and authenticated host session wired in, the generated call is:

```rust
let rows = api
    .items(
        client::ItemsInput {},
        client::ItemsCtx {
            owner: session.owner(),
        },
    )
    .await?;
```

The result is a typed list of `ItemView`, including its optional nested parent.
The host supplies authenticated context; the engine enforces the declared modeled
scope. The tutorial proves another owner's lookup is absent, then renames a stored
column while preserving rows, relation IDs and the public projection. This is a
fabricated tutorial, not production-adoption or productivity evidence.

Go directly to [installation](installation.md), then choose the
[embedded Rust tutorial](embedded-tutorial.md) or the
[standalone HTTP tutorial](standalone-tutorial.md). After installing the matching
CLI, an empty directory reaches embedded create/read with:

```sh
based init --mode embedded
based migrate apply --database-url local.db
cargo run
```

Embedded setup needs Rust 1.94+, Cargo, Git and platform C build tools. Standalone
uses the same engine through HTTP behind a trusted auth edge; its runnable demo
needs Python's standard library. Both paths have clean-project first-use and
schema-edit automation. Before a release tag exists, the installation guide's
explicitly pinned public Git route is available; do not use an invented asset URL.
No signup or volunteer commitment is required to evaluate the source.

SQLx, Diesel's async companion and SeaORM already offer async access. The benefit
here is reviewing modeled policy, projection and callables together, with another
language and an engine to evaluate. Large operations showed meaningful
[measured runtime overhead](runtime-performance.md); explicit isolated generation
is the default after the [consumer build-cost gate](consumer-build-cost.md).
There is no general speed or AI-productivity claim.

The initial [support matrix](support-policy.md) covers bundled SQLite, MariaDB 11.4
and PostgreSQL 16. Existing databases can use [one-shot model import](import-existing-database.md)
while retaining their migration owner. Source/runtime/extension are AGPL-3.0-only,
with [separate generated-output terms](../LICENSE-GENERATED.md).
[Short answers](reveal-faq.md) explain costs, alternatives and escape hatches.

If evaluation hits a problem, [report an issue](https://github.com/oscscull/baseddsl/issues/new/choose)
with the version, relevant schema/query, expected/actual result and a safe minimal
reproduction. [Sensitive reports](../SECURITY.md) go privately. Recurring use and
specific barriers will guide follow-up; no telemetry or private database collection
is required. Support is best effort.

## Verification before posting

- Use the final public release's exact version/commit and actual native/VSIX asset
  links. Verify checksums and matching CLI/LSP/extension versions, then replay both
  first-use paths and the schema edit from a clean environment on that revision.
- The [existing onboarding gate](onboarding-gate.md) exercises this public starter,
  typed call, ownership boundary and edit. Its source-consumer gate tests the
  minimum Rust toolchain. A changed final tag needs its own successful gates.
- Review the title and venue with the owner and obtain explicit posting instruction.
  Continue with the [bounded manual feedback review](post-launch-review.md).
