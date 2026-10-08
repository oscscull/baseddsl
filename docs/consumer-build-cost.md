# Cargo generation build-cost decision

**Decision for the starter: use explicit `generated/` files with `include!`.** The
OUT_DIR helper stays optional and experimental. The measured repeated edit-loop
penalty is significant for this small consumer, and the initial cold series also
had substantial unexplained timing drift. This evidence does not justify making
the new build stage the default. This is the pivot required by
[#54](https://github.com/oscscull/baseddsl/issues/54), provided for owner review
before the initializer selects its workflow. No numeric acceptance tolerance was
invented, and no future optimization is required to opt out of the costly path.

Explicit generation keeps configured paths, ownership checks, unchanged mtimes,
formatter isolation and convenient `based gen all` / `based gen all --check`.
See [generated artifacts](generated-artifacts.md). The
[optional helper](cargo-generation.md) remains available for users who deliberately
choose its tradeoff; it is not a dependency of ordinary embedded or standalone
consumers.

## Experiment

Measured on 2026-10-09 JST (2026-10-08 UTC), Apple M4 Max, 14 CPU cores, 36 GiB RAM,
macOS 27.0.1, Rust/Cargo 1.99.0, 14 Cargo jobs. Compiler/runtime source was
`f394dc09f089dfe683d763363d3d371565be6c4e` (the #80 prototype). Both series used the
same locked consumer, features and default consumer dev profile; workspace
profiles were not inherited. Raw metadata records the lock hashes, compiler
identity, CLI hash/version/profile, unit features, artifact sizes and all samples.

The two embedded variants use SQLite and production `id-gen`, the same schema,
Rust application, query/result JSON and generated client payload. One includes
`OUT_DIR/client.rs` from the optional build script; the other has no build script
and includes `generated/client.rs`. The separate direct SQLx variant selects the
same SQLx SQLite/Tokio features, UUID support and serialized query result while
omitting the Based runtime/compiler. It is a positioning baseline, not a claim
that plain SQLx provides the DSL's closed typed API or runtime policies.

Each variant/trial received a fresh scratch target directory. All 18 cold builds
reported every Cargo unit as newly compiled. Registry sources were prefetched;
timed commands were locked and offline, with no compiler wrapper. Ordinary OS
caches were not flushed. Host build-dependency compilation, checking/emission,
process startup and application compilation/relinking are included. An installed
release-mode CLI is the explicit-generation prerequisite: its installation/build
is outside consumer timings, while **every needed regeneration invocation is
counted**, including regeneration of the committed client before a cold build.
This reflects using the separately distributed CLI; source-install/download
cost is not hidden inside an application compilation number.

Each warmed target then measured an unchanged build, a Rust function-body edit,
a BSL order change with unchanged client API, a new callable in a new directory,
and its deletion. Applications were executed untimed after each step and asserted
the same SQLite result. The helper fixture records actual checking/publication;
Cargo JSON records Rust units rebuilt. Invalidation is verified rather than
inferred from timing.

## Repeat series

Medians of three fresh-target trials, with observed ranges. Explicit totals
include regeneration when needed. The direct baseline does not consume BSL, so
its BSL columns are inapplicable.

| Build scenario | OUT_DIR median (range), s | Explicit median (range), s | Added seconds | Relative change | Direct SQLx median, s |
|---|---:|---:|---:|---:|---:|
| cold | 9.988 (9.894–10.000) | 9.588 (9.585–9.638) | +0.400 | +4.2% | 7.419 |
| unchanged | 0.658 (0.656–0.676) | 0.094 (0.093–0.095) | +0.564 | +600.5% | 0.084 |
| rust_edit | 0.378 (0.372–0.380) | 0.357 (0.352–0.361) | +0.022 | +6.0% | 0.308 |
| bsl_edit | 0.668 (0.665–0.674) | 0.095 (0.094–0.102) | +0.574 | +607.0% | n/a |
| bsl_add | 0.882 (0.881–0.885) | 0.567 (0.567–0.571) | +0.315 | +55.5% | n/a |
| bsl_delete | 0.878 (0.872–0.880) | 0.569 (0.567–0.570) | +0.309 | +54.3% | n/a |

| Explicit scenario | Generation median, s | Cargo median, s |
|---|---:|---:|
| cold | 0.008 | 9.580 |
| bsl_edit | 0.009 | 0.086 |
| bsl_add | 0.027 | 0.544 |
| bsl_delete | 0.019 | 0.549 |

| Variant | Cold Cargo units | Target unique logical bytes | Application bytes |
|---|---:|---:|---:|
| out_dir | 180 | 808691683 | 19092136 |
| explicit | 159 | 725058079 | 19093320 |
| sqlx | 125 | 388861227 | 8861320 |


In this repeat series, the cold penalty is modest: +0.40s / +4.2%. The more
important starter tradeoff is the repeated edit loop: the query-only BSL change
adds +0.57s to a 0.095s explicit regeneration/build, and additions/deletions add
about +0.31s. The first unchanged build also adds +0.56s. These are visible
absolute costs in this deliberately small app; percentages on subsecond baselines
are not a universal performance claim. The Rust-only difference (+0.022s) is
small and does not establish a meaningful general penalty.

Watching the generated output triggered one cache-hit build-script rerun after
initial creation in these measurements and can do so after output-changing generation. That run does **not** invoke the
BSL compiler or client emitter. Its startup/hash and Cargo relink cost remain in
the unchanged column. Rust-only edits do not run the build script in the dedicated
schema-directory fixture. Query-only edits check BSL while preserving identical
client bytes/mtime; the helper still causes application recompilation. The
explicit path regenerates the unchanged client and leaves Cargo's application
unit fresh. Added/deleted callables change the client in both paths.

The OUT_DIR cold graph has 21 extra Cargo units and about 79.8 MiB of additional
unique logical target artifacts (+11.5%). `based-codegen` is compiled twice:
`client` enabled with host build-dependency debug information disabled, and the
runtime SQL-only variant with normal dev debug information. The AST/parser/sema
units are shared in this native graph; they are not all compiled twice. Additional
host crates include `based-build`, `based-project`, the artifact publisher and
cache digest dependencies. This is actual unit evidence, consistent with Cargo's
[resolver feature separation](https://doc.rust-lang.org/cargo/reference/resolver.html#feature-resolver-version-2).
Application binary sizes are essentially equal; the 1.2 KiB difference does not
support a runtime/binary-size advantage claim.

Direct SQLx cold compilation is 7.419s in the repeat series, versus 9.588s for
explicit Based (+2.17s / +29.2%). This retains the overall build-positioning
comparison and does not treat SQLx's own cost as permission to add another stage.
The fixture is a single read with JSON serialization; broader applications,
platforms, cross-compilation and release-profile consumer builds were not timed.

## Initial series and uncertainty

The first series must not be pooled into one apparently precise ratio:

| Initial trial | OUT_DIR cold, s | Explicit cold, s | Added seconds | Direct SQLx cold, s |
|---|---:|---:|---:|---:|
| 1 | 91.205 | 83.120 | +8.085 | 55.863 |
| 2 | 90.455 | 81.868 | +8.587 | 56.499 |
| 3 | 21.645 | 10.389 | +11.256 | 7.709 |

The baseline and incremental times shifted substantially during that series,
despite fresh target directories and matching unit/features/profile evidence.
The cause was not isolated, so these samples do not establish a stable cold
ratio or a guaranteed 8–11s penalty. They are retained rather than discarded.
The immediate repeat series was steadier, with all cold OUT_DIR builds between
9.894 and 10.000s and explicit builds between 9.585 and 9.638s. Even that result
is limited to this host and its cache conditions. The first series' uncertainty
is an additional reason to retain the existing default rather than claim
unconditional owner acceptance of a new build stage.

Raw evidence:
[initial series](../benchmarks/consumer-build/measurements/apple-m4-max.json),
[repeat series](../benchmarks/consumer-build/measurements/apple-m4-max-repeat.json).
The initial Cargo path-package ID fragments omitted implicit package names; those
labels were resolved from their recorded library targets. Timing samples are
retained as measured.

## Reproduce and verify

Follow the [benchmark protocol](../benchmarks/consumer-build/README.md). The
measurement scripts mutate only scratch projects and remove only their own
scratch directories; they never restore developer files with `git checkout`.
`make ci-consumer-build` checks fixture freshness, matching result semantics,
compiler invalidation, formatting and per-variant Clippy. CI checks correctness;
there is no flaky wall-clock threshold. Existing helper lifecycle tests cover
invalid inputs, output recovery and shared schema/application directory layouts.

The initializer and tutorials should use the explicit workflow. Reconsidering a
future helper default requires new repeated evidence and owner review; the
current starter does not wait for that hypothetical optimization.

The archived measurements retain their recorded source/lock hashes. The current
fixture lock records the runtime's explicit SQLite safety-floor dependency; both
measured benchmark fixtures already selected bindings 0.37.0. CI rechecks parity
and invalidation on the current fixture without rewriting historical timings or
claiming a new performance measurement.
