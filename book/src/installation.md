# Installation

Install matching CLI and language-server binaries from the same
[release](https://github.com/oscscull/baseddsl/releases). Check the archive against
`SHA256SUMS` before extracting it and adding its directory to PATH:

```sh
sha256sum --check --ignore-missing SHA256SUMS  # Linux
shasum -a 256 --check SHA256SUMS              # macOS
based --version
based-lsp --version
```

On Windows, compare `Get-FileHash -Algorithm SHA256` with the checksum entry,
then use `Expand-Archive` and add the extracted directory to your user PATH.

| Target | Minimum host |
|---|---|
| Linux x86_64 | glibc 2.35 (Ubuntu 22.04) |
| Linux arm64 | glibc 2.39 (Ubuntu 24.04) |
| macOS arm64 or Intel | macOS 15 |
| Windows x86_64 | Windows 10 / Server 2022 |

Native tools need no Rust installation. Building a Rust application or installing
from source needs Rust 1.94+, Cargo, Git, and platform C build tools for bundled
SQLite: Xcode command-line tools, GCC/build essentials, or MSVC Build Tools.

Until a version is published, use an explicitly reviewed full commit:

```sh
cargo install --git https://github.com/oscscull/baseddsl --rev FULL_COMMIT --locked based-cli
cargo install --git https://github.com/oscscull/baseddsl --rev FULL_COMMIT --locked based-lsp
```

Pin both tools and the runtime to the same source revision. Keep each native
version in its own directory; selecting another directory on PATH rolls back the
tools, but does not undo database changes. See [upgrading](reference.md).
