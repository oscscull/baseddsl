# Consumer build-cost gate

This benchmark compares the optional OUT_DIR helper with the **same** application
using explicit, included client generation. The direct SQLx variant is a separate
positioning baseline. The decision and repeated measurements are documented in
[consumer build costs](../../docs/consumer-build-cost.md).

The fixture selects SQLite and production `id-gen` for both embedded variants.
They share source code, result semantics, exact lockfile, dependency versions,
features and the consumer's default dev profile. The only workflow change is the
optional build dependency/build script and the client include location. The
explicit variant removes `build.rs` entirely. Generated Rust payloads are checked
for equality, excluding only the regeneration-command comment.

Direct SQLx uses the same locked SQLx SQLite/Tokio features, selected UUID support,
JSON serialization and query result, with no Based runtime/compiler. Its BSL
edit/add/delete columns are intentionally inapplicable. Each built application
creates an in-memory SQLite table and asserts the same typed/read/serialized row;
execution is untimed and checks build-result semantics, not runtime performance.

## Run

Prerequisites: the supported Rust toolchain, Cargo, a C compiler for bundled
SQLite, Python 3, and a built/installed matching `based` executable. The correctness
gate also uses rustfmt and Clippy. Prepare tools and dependencies outside timing:

```sh
cargo build --release --locked -p based-cli --no-default-features
cargo fetch --locked --manifest-path benchmarks/consumer-build/fixture/Cargo.toml
python3 benchmarks/consumer-build/verify.py --based target/release/based
```

Then stop other compilation before timing:

```sh
ci/measure-incremental.sh --based "$PWD/target/release/based" \
  --output "$PWD/benchmarks/consumer-build/measurements/local.json" --trials 3 --jobs 14
python3 benchmarks/consumer-build/report.py \
  benchmarks/consumer-build/measurements/local.json
```

Every variant/trial gets a fresh scratch project and target directory. Registry
sources are prefetched and timed builds run `--locked --offline`; no compiler
wrapper/cache is allowed. This is cold **compilation**, with a warm source-download
cache and ordinary OS page caches. All host build dependencies and native library
compilation are included. Installing/building the CLI prerequisite is separate;
its version, profile, binary size and hash are recorded, and each needed explicit
generation invocation includes its process startup, checking/emission and output
publication. Even the explicit cold build counts regeneration of the committed
client rather than treating it as free. Unchanged and ordinary Rust builds need
no explicit regeneration.

The order alternates across trials. After cold compilation, each corresponding
target directory is reused for an unchanged build, a Rust function-body edit,
a BSL order change that leaves the client API unchanged, an added callable in a
new schema directory, and its deletion. The helper fixture records whether BSL
checking and output replacement actually ran. Unchanged and Rust-only builds
must skip the BSL compiler and emitter; an output watch can cause a cheap
build-script cache-hit rerun, and that startup/hash/relink cost stays in the
measurements. Assertions distinguish it from generation rather than inferring
invalidation from elapsed time.

The report retains cold Cargo compiler units/features/output sizes, units rebuilt
in each incremental step, target artifact size, application size, and separate
explicit-generation/Cargo/combined times. Scratch directories are deleted after
measurement; repository sources and developer target directories are never
edited, cleaned or restored. `ci/measure-incremental.sh` now delegates to this
safe protocol instead of modifying a workspace source file.

`make ci-consumer-build` runs correctness, invalidation, formatter and per-variant
Clippy checks; CI imposes no wall-clock threshold. The benchmark is deliberately
small. These results establish this consumer's costs on the measured host, not
a universal build-time ratio, cross-target cost or installation/download speed.
