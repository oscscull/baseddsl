#!/usr/bin/env python3
"""Replay fresh initializer starts and the explicit schema-update contract."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "ci/release"))
from metadata import git
from source import mirror


def invoke(args, directory, environment, success=True):
    result = subprocess.run([str(arg) for arg in args], cwd=directory, env=environment,
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert (result.returncode == 0) == success, (args, result.stdout, result.stderr)
    return result.stdout + result.stderr


def identity(output):
    records = {}
    for line in output.splitlines():
        for label in ("created", "read"):
            if line.startswith(f"{label}:"):
                records[label] = json.loads(line.split(":", 1)[1])
    created, rows = records["created"], records["read"]
    assert uuid.UUID(created["id"]).version == 4
    assert any(row["id"] == created["id"] for row in rows), records


def fingerprint(directory):
    return {str(path.relative_to(directory)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in directory.rglob("*") if path.is_file() and "target" not in path.parts}


def columns(app):
    with sqlite3.connect(app / "local.db") as database:
        return [row[1] for row in database.execute("PRAGMA table_info(Item)")]


def fresh(based, directory, mode, dialect, environment):
    output = invoke([based, "init", directory, "--mode", mode, "--dialect", dialect], ROOT, environment)
    assert "1. based migrate apply" in output
    assert "2. " in output and "written" not in output
    assert not (directory / "local.db").exists()
    assert not (directory / "build.rs").exists()
    invoke([based, "gen", "all", "--check"], directory, environment)
    invoke([based, "migrate", "verify"], directory, environment)
    before = fingerprint(directory)
    error = invoke([based, "init", directory, "--mode", mode], ROOT, environment, success=False)
    assert "empty directory" in error
    assert fingerprint(directory) == before


def cargo(app, target, environment, operation):
    return invoke(["cargo", operation, "--manifest-path", app / "Cargo.toml", "--target-dir", target,
                   *(["--", "-D", "warnings"] if operation == "clippy" else [])], app, environment)


def sqlite_start(based, directory, mode, target, environment):
    fresh(based, directory, mode, "sqlite", environment)
    invoke([based, "migrate", "apply", "--database-url", "local.db"], directory, environment)
    if mode == "embedded":
        invoke(["cargo", "fmt", "--check"], directory, environment)
        demo = lambda: cargo(directory, target, environment, "run")
    else:
        demo = lambda: invoke([sys.executable, "demo.py"], directory, environment)
    identity(demo())
    schema = directory / "schema/item.bsl"
    schema.write_text(schema.read_text().replace("  name: text", "  name: text\n  description: text?"))
    invoke([based, "gen", "all"], directory, environment)
    invoke([based, "migrate", "gen"], directory, environment)
    invoke([based, "migrate", "verify"], directory, environment)
    assert "description" not in columns(directory), "generation silently applied a migration"
    invoke([based, "migrate", "apply", "--database-url", "local.db"], directory, environment)
    assert "description" in columns(directory)
    identity(demo())
    if mode == "embedded":
        cargo(directory, target, environment, "clippy")
        manifest = (directory / "Cargo.toml").read_text()
        assert 'git = "https://github.com/oscscull/baseddsl.git"' in manifest
        assert '"id-gen"' in manifest and 'path =' not in manifest


def collision(based, scratch, environment):
    existing = scratch / "existing-rust"
    existing.mkdir()
    (existing / "Cargo.toml").write_text('[package]\nname="existing"\nversion="0.1.0"\n')
    before = fingerprint(existing)
    output = invoke([based, "init", existing, "--mode", "embedded"], ROOT, environment, success=False)
    assert "existing Rust projects" in output
    assert fingerprint(existing) == before
    file = scratch / "source-file"
    file.write_text("preserve source")
    invoke([based, "init", file, "--mode", "embedded"], ROOT, environment, success=False)
    assert file.read_text() == "preserve source"
    alias = scratch / "alias"
    alias.symlink_to(existing, target_is_directory=True)
    invoke([based, "init", alias, "--mode", "standalone"], ROOT, environment, success=False)
    assert fingerprint(existing) == before


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    options = parser.parse_args()
    based = options.based.resolve()
    with tempfile.TemporaryDirectory(prefix="based-init-consumers-") as scratch:
        root = Path(scratch).resolve()
        # Preserve the public pinned manifest. Only this harness's Git transport is
        # redirected to a committed local mirror, including PR merge commits.
        source_url = mirror(ROOT, git("rev-parse", "HEAD"), root / "source.git")
        environment = dict(os.environ, BASED=str(based), CARGO_NET_GIT_FETCH_WITH_CLI="true",
                           GIT_CONFIG_COUNT="1", GIT_CONFIG_KEY_0=f"url.{source_url}.insteadOf",
                           GIT_CONFIG_VALUE_0="https://github.com/oscscull/baseddsl.git")
        environment.pop("DATABASE_URL", None)
        environment.pop("BASED_DATABASE_URL", None)
        target = root / "target"
        for mode in ("embedded", "standalone"):
            sqlite_start(based, root / f"{mode}-sqlite", mode, target, environment)
            for dialect in ("mariadb", "postgres"):
                app = root / f"{mode}-{dialect}"
                fresh(based, app, mode, dialect, environment)
                if mode == "embedded":
                    invoke(["cargo", "fmt", "--check"], app, environment)
                    cargo(app, target, environment, "clippy")
        collision(based, root, environment)
    print("Initializer fresh starts, update cycles, driver builds, UUIDs, and collisions passed")


if __name__ == "__main__":
    main()
