# External onboarding gate

`make ci-onboarding` runs the published embedded and standalone lessons in fresh
temporary directories outside the repository. It reuses the initializer and
tutorial consumer harnesses rather than a second schema or driver matrix.
The normal CI job builds its candidate CLI once. Release dry runs instead extract
the matching Linux native archive, then run the same lesson harnesses; candidate
collection fails if either first run or upgrade fails.

The embedded source-consumer job retains the Rust **1.94** tool build check, then
uses the extracted native CLI to initialize an external Rust application. Init's
public Git dependency is pinned to that tool's exact source commit. CI redirects
only Git transport to a temporary bare mirror of that commit, including unpublished
PR merge commits. The manifest and Cargo.lock retain Git source identity; there
are no checkout-relative dependencies. The fresh project has no build script,
previous generated client, or preconfigured database. A PATH sentinel rejects any
attempt to use an unrelated installed `based`.

First create/read follows exactly `init`, explicit migration application, and
`cargo run`, with no manual dependency or generated-file edits. The gate checks
owner-scoped parent/child output, a real SQLite file, and the pinned lockfile.
After the documented rename, a freshness check detects stale SQL, explicit
generation prepares artifacts, rendering reviews the migration, and separate
application preserves rows/owner/parent IDs. Regenerated typed calls run and pass
consumer Clippy. A custom formatter configuration formats only host source;
the committed generated artifact stays byte-identical, including generation and
freshness checks from a subdirectory.

Missing CLI/Cargo prerequisites, invalid manifest configuration, user-owned output
collisions, and an unavailable SQLite path produce asserted actionable failures.
This does not replace live MariaDB/PostgreSQL CI: their configured service failures
still fail the existing jobs and are never converted to skips.

The standalone collector packages the matching Python lesson zip, extracts it
into a blank initialized project, and uses only the native CLI and Python standard
library. It exercises the real authenticated edge and documented client, forged
context/shard headers, scoped relations, named callback denial/unavailability,
database-backed replay after process restart, stale conditional writes, and the
explicit rename migration. No Rust/npm consumer build is required for that path.

[`ci/onboarding/commands.py`](../ci/onboarding/commands.py) owns the tested command
lists and explicitly checks both published tutorial command blocks. It also
checks the guarded-write snippet against its canonical source. The same command
lists drive the embedded harness. Installation is exercised by archive extraction
in the release jobs, not by trusting PATH. Platform-specific checksum/extraction
instructions remain in the [installation guide](installation.md); this gate does
not claim a full OS × driver onboarding matrix.

See the [embedded](embedded-tutorial.md) and [standalone](standalone-tutorial.md)
tutorials for user instructions, [generated clients](generated-rust.md) for the
formatting boundary, and [release preparation](releasing.md) for owner publication.
