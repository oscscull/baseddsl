# Release dry runs and owner publication

The [release workflow](../.github/workflows/release.yml) separates native packaging
from publication. Relevant PRs and manual runs build/smoke all five native targets,
exercise the Rust 1.94 source-install floor and a fresh exact-commit Git library
consumer, collect versioned archives and generate SHA-256 inventories. These runs
upload Actions artifacts; they do not create a tag or publish a release.

Native job steps are build/setup wrappers. The portable helpers under
[`ci/release/`](../ci/release/) validate inherited package/license/toolchain
metadata, matching extension version, binary version/commit identities, archive
contents, checksums and extracted executable behavior. Each package is verified
on its native host using offline checking/generation and explicit SQLite
migration application. No database server is required. The source consumer uses
a temporary bare Git mirror with just the selected release commit/history, so
its Cargo manifest has no checkout-relative path dependencies.

## Local native dry run

Use Python 3.12+, Rust 1.94+, Cargo/Git and platform C tools. Build both tools with
production defaults. Obtain your native triple from `rustc -Vv` (`host:`).
For example, on Apple silicon:

```sh
cargo build --release --locked --target aarch64-apple-darwin -p based-cli -p based-lsp
python3 ci/release/package.py \
  --binary-dir target/aarch64-apple-darwin/release \
  --target aarch64-apple-darwin --output /tmp/based-release-dry-run
python3 ci/release/source_consumer.py \
  --based target/aarch64-apple-darwin/release/based
```

The output directory must not already contain that named archive. Package/source
checks use the current exact commit. A local work-in-progress packaging check may
pass `--allow-dirty`; its manifest records that status and the artifact is not a
publishable release. The source-consumer check uses committed source, so commit
candidate changes before using it to verify the candidate's Git distribution.
Build again after a commit change so the tools' embedded identities match.

For Windows native artifacts, set `RUSTFLAGS=-C target-feature=+crt-static` before
building, use `x86_64-pc-windows-msvc`, and select the `.exe` binaries. The workflow
uses this setting and native MSVC tools. macOS release jobs set deployment target
15.0; Linux jobs use their stated glibc build hosts. See the exact
[installation matrix](installation.md).

The archive includes both binaries, LICENSE, RELEASE-NOTES.md, SOURCE.txt and
manifest.json. Its filename is `based-VERSION-TARGET.tar.gz` or `.zip` on Windows.
`SHA256SUMS` covers every final uploaded asset. The collector rejects mismatched
versions, commits, dirty native sources and duplicate target identities.

The [external onboarding gate](onboarding-gate.md) runs both published tutorials
using extracted candidate tools. The collector also packages `based-standalone-tutorial-VERSION.zip`, verifies its
matching clean source manifest, and executes the extracted lesson against the
Linux x64 native CLI. That path needs only Python, with no Rust/npm consumer build.
The lesson zip and VSIX are included in the final combined inventory.

## Before an owner-approved version tag

Complete the [v1 readiness issue](https://github.com/oscscull/baseddsl/issues/73)
and its release checklist. That includes the external onboarding gate, support
and security/contribution policy, documented compatibility limits, licensing and
owner approval of the public promise. Confirm the matching VSIX/editor gate
[#57](https://github.com/oscscull/baseddsl/issues/57) before public distribution.
Keep the candidate at its existing 0.1.12 version until the owner selects the
release version; update workspace and extension metadata, lockfiles, notes and
installation examples together when changing it.

All intended code must be reviewed and merged by an explicitly authorized human;
these preparation PRs never merge themselves. Run the dry-run workflow against
the exact proposed release source and inspect every native artifact/version,
checksums and source-consumer result. Read the existing decimal and conditional
helpdesk compatibility notes as well as the regenerated-client instructions.

## Owner-only tag and publication steps

Only after the checklist is approved, create and push the selected version tag
from the reviewed clean commit (example candidate name shown here):

```sh
TAG=v0.1.12
git tag -a "$TAG" -m "Based $TAG"
git push origin "$TAG"
```

Tag pushes run the same checks with an exact tag/workspace version match. They
create **a draft** GitHub release only after all jobs pass. A manual workflow can
also create a draft when explicitly run from an existing version tag with
`create_draft=true`; a branch dry run cannot publish or create a tag. Retrying an
existing draft requires inspecting/replacing its assets deliberately, rather
than silently changing an already published release.

Inspect/download the draft's assets, verify SHA256SUMS, check both `--version`
lines and confirm source/upgrade instructions. Publication is a separate owner
action after the final checklist:

```sh
gh release edit "$TAG" --draft=false
```

This work does not execute these owner-only tag/publication commands. No registry
or marketplace account is required. Do not overwrite published tags or replace
public binaries under a version; publish a new version with clear upgrade notes.
For a bad release, mark it with a clear advisory and direct users to a known good
pinned version. Tool rollback and database rollback are separate operations.

The `vsix` job installs the packaged extension in an isolated VS Code profile and
checks diagnostics, hover, completion, and rename using the Linux x64 prebuilt
LSP archive from the same run. Collection verifies the VSIX version, license,
and required runtime files, then includes it in the combined checksums. This
produces a locally installable extension; it does not publish to the Marketplace.
