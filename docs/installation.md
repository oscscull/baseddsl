# Versioned installation

The current source version is **0.1.12**, an evaluation candidate. Public tags and
release assets are produced only after the v1 readiness/owner checklist. Until
then, evaluate an explicitly pinned source commit from a reviewed PR or a dry-run
artifact; do not treat a proposed release link as an already published asset.

## Native CLI and language server

The release workflow produces one archive containing matching `based` and
`based-lsp`, LICENSE, release notes, an exact source link and `manifest.json` with
version, commit, toolchain, target and binary identities. Install both binaries
from the same archive. Download the archive and `SHA256SUMS` from the
[release for the chosen version](https://github.com/oscscull/baseddsl/releases).
Verify the archive hash before extraction:

```sh
# Linux (from the download directory):
sha256sum --check --ignore-missing SHA256SUMS
# macOS:
shasum -a 256 --check SHA256SUMS
```

On macOS, retain the archive's matching checksum line locally when checking one
asset; `shasum` reports other absent assets separately. On Windows PowerShell,
compare `(Get-FileHash .\based-0.1.12-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash`
with the entry in `SHA256SUMS`, then use `Expand-Archive` to a versioned directory.

| Native artifact target | Supported evaluation host | Build/smoke runner |
|---|---|---|
| `x86_64-unknown-linux-gnu` | Linux x86_64, glibc 2.35+ (Ubuntu 22.04+) | Ubuntu 22.04 |
| `aarch64-unknown-linux-gnu` | Linux arm64, glibc 2.39+ (Ubuntu 24.04+) | Ubuntu 24.04 arm64 |
| `aarch64-apple-darwin` | macOS 15+ Apple silicon | macOS 15 arm64 |
| `x86_64-apple-darwin` | macOS 15+ Intel | macOS 15 Intel |
| `x86_64-pc-windows-msvc` | Windows x86_64, Windows 10+/Server 2022 | Windows Server 2022, static CRT |

The exact native jobs use
[documented runner labels](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job).
These are the initial prebuilt support boundaries; other hosts can try pinned
source builds. Linux should have its ordinary system certificate bundle installed
for verified TLS. Bundled SQLite needs no database server. Native artifacts do
not require Rust or npm to execute; embedded applications still need Rust to
compile their own code. Windows archives use a static C runtime.

For example, after the candidate tag is published, extract a Unix archive into
a versioned directory and add it to PATH:

```sh
mkdir -p "$HOME/.local/share/based/0.1.12"
tar -xzf based-0.1.12-aarch64-apple-darwin.tar.gz \
  -C "$HOME/.local/share/based/0.1.12"
export PATH="$HOME/.local/share/based/0.1.12:$PATH"
based --version
based-lsp --version
```

Select the archive for your host, and persist PATH using your shell's normal
configuration. Windows users can add the extracted directory to their user PATH.
Keep each version in its own directory; changing PATH or the editor's existing
`basedls.serverPath` pins or rolls back the tools without overwriting another
version. See [release notes](release-notes.md) before changing runtime/client or
database state. A binary rollback does not reverse database migrations.

## Pinned source fallback

Source installation needs Rust **1.94+**, Cargo, Git and the platform's C build
tools for bundled SQLite (Xcode command-line tools on macOS, gcc/build essentials
on Linux, or MSVC Build Tools on Windows). The exact minimum toolchain is exercised
by the release dry run. SQLx 0.9 declares Rust 1.94; the workspace metadata now
matches that requirement. Use a stable compatible toolchain and a published tag:

```sh
cargo install --git https://github.com/oscscull/baseddsl --tag v0.1.12 --locked based-cli
cargo install --git https://github.com/oscscull/baseddsl --tag v0.1.12 --locked based-lsp
```

Before that tag exists, replace `--tag v0.1.12` with `--rev FULL_VERIFIED_COMMIT`
from the reviewed source or dry-run manifest. Pin the same full commit for both
commands, record it, and compare `--version` after installing. Do not recommend
floating `main`. Tools installed by Cargo normally live in `$HOME/.cargo/bin`.
Use an explicit `--root` for side-by-side source installations if you want to
retain a rollback version.

## Library dependency outside this repository

After the matching source tag is published, an independent application can use:

```toml
[dependencies]
based-runtime = { git = "https://github.com/oscscull/baseddsl", tag = "v0.1.12", features = ["sqlite", "id-gen"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Before publication, use `rev = "FULL_VERIFIED_COMMIT"` instead of `tag`. Internal
workspace compiler dependencies resolve from that same Git source; the app needs
no repository-relative path dependencies. Commit the application's Cargo.lock.
Generated decimal clients additionally require `bigdecimal = "0.4"`; see the
specific [API upgrade notes](release-notes.md). Choose the runtime driver/features
for your deployment; production UUID/ULID generation uses `id-gen`, and an
embedded app needs no `serve` feature.

Generate the client using the matching CLI and follow
[isolated artifact generation](generated-artifacts.md). Runtime schema assets and
reviewed migrations remain explicit. The
[external source-consumer gate](../ci/release/source_consumer.py) uses an isolated
Git mirror and exact commit, compiles a fresh application with this dependency
route, and executes its typed SQLite call. It does not rely on installed registry
packages or repository-relative dependencies.

Registry and marketplace publication can be added separately. This initial
route is versioned native artifacts plus pinned Git source.
