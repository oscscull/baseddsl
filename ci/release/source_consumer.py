#!/usr/bin/env python3
"""Prove pinned Git library consumption from a fresh project outside the repository."""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

from metadata import ROOT, git
from source import mirror


def probe(based, commit, cargo):
    fixture = ROOT / "benchmarks/consumer-build/fixture"
    with tempfile.TemporaryDirectory(prefix="based-release-source-") as scratch:
        directory = Path(scratch)
        source_url = mirror(ROOT, commit, directory / "source.git")
        app = directory / "consumer"
        shutil.copytree(fixture, app, ignore=shutil.ignore_patterns("target", "Cargo.lock", "build.rs"))
        # No repository-relative paths remain in the consuming Cargo manifest.
        manifest = f'''[package]
name = "pinned-source-consumer"
version = "0.0.0"
edition = "2021"
rust-version = "1.94"
[workspace]
[features]
embedded = []
direct = []
cargo-generation = []
[dependencies]
based-runtime = {{ git = "{source_url}", rev = "{commit}", features = ["sqlite", "id-gen"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
'''
        (app / "Cargo.toml").write_text(manifest)
        subprocess.run([str(based), "gen", "client"], cwd=app, check=True)
        subprocess.run([cargo, "run", "--features", "embedded", "--target-dir", str(directory / "target")], cwd=app, check=True)
        locked = (app / "Cargo.lock").read_text()
        assert f"#{commit}" in locked, "the library did not resolve the pinned commit"
        subprocess.run([cargo, "clippy", "--locked", "--features", "embedded", "--target-dir", str(directory / "target"), "--", "-D", "warnings"], cwd=app, check=True)
    print(f"fresh pinned source consumer passed: {commit}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    parser.add_argument("--cargo", default="cargo")
    options = parser.parse_args()
    probe(options.based.resolve(), git("rev-parse", "HEAD"), options.cargo)


if __name__ == "__main__":
    main()
