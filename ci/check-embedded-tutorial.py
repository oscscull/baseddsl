#!/usr/bin/env python3
"""Verify the documented data-preserving rename in a fresh external consumer."""
import argparse
import os
from pathlib import Path
import sqlite3
import tempfile
import sys

from initializer import ROOT, cargo, identity, invoke
from metadata import git
from source import mirror

sys.path.insert(0, str(ROOT / "ci/onboarding"))
from commands import EMBEDDED_START, UPGRADE, arguments, verify as verify_commands
from failures import verify as verify_failures
from formatting import verify as verify_formatting
from prerequisites import require, verify_missing
from environment import forbid_path_cli


def records(app, column):
    assert column in ("name", "title")
    with sqlite3.connect(app / "local.db") as database:
        return list(database.execute(f"SELECT id, {column}, owner, parent_id FROM item ORDER BY id"))


def rename_source(path):
    source = path.read_text()
    source = source.replace("  name: text", '  title: text @was("name")')
    source = source.replace("\n  name\n", "\n  name = title\n")
    source = source.replace("parent { id, name }", "parent { id, name = title }")
    source = source.replace("name = $name", "title = $name")
    path.write_text(source)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    options = parser.parse_args()
    based = options.based.resolve()
    try:
        require(based)
    except RuntimeError as error:
        parser.error(str(error))
    verify_commands(ROOT)
    with tempfile.TemporaryDirectory(prefix="based-embedded-tutorial-") as scratch:
        root = Path(scratch).resolve()
        source_url = mirror(ROOT, git("rev-parse", "HEAD"), root / "source.git")
        environment = dict(os.environ, CARGO_NET_GIT_FETCH_WITH_CLI="true", GIT_CONFIG_COUNT="1",
                           GIT_CONFIG_KEY_0=f"url.{source_url}.insteadOf",
                           GIT_CONFIG_VALUE_0="https://github.com/oscscull/baseddsl.git")
        environment.pop("DATABASE_URL", None)
        environment.pop("BASED_DATABASE_URL", None)
        environment = forbid_path_cli(root / "no-installed-cli", environment)
        app = root / "app"
        target = root / "target"
        verify_missing(based, root / "no-tools")
        app.mkdir()
        output = invoke(arguments(EMBEDDED_START[0], based), app, environment)
        assert "1. " + EMBEDDED_START[1] in output and "2. " + EMBEDDED_START[2] in output
        assert not (app / "local.db").exists()
        manifest = (app / "Cargo.toml").read_text()
        assert 'git = "https://github.com/oscscull/baseddsl.git"' in manifest and "path =" not in manifest
        assert not (app / "build.rs").exists()
        invoke(arguments(EMBEDDED_START[1], based), app, environment)
        identity(cargo(app, target, environment, "run"))
        assert f'#{git("rev-parse", "HEAD")}' in (app / "Cargo.lock").read_text()
        before = records(app, "name")
        assert len(before) == 2
        rename_source(app / "schema/item.bsl")
        stale = invoke([based, "gen", "all", "--check"], app, environment, success=False)
        assert "stale" in stale.lower(), stale
        for command in UPGRADE[:3]:
            invoke(arguments(command, based), app, environment)
        assert records(app, "name") == before, "generation silently changed the database"
        migration = app / "migrations/0002_rename_item_title/up.mig"
        assert "rename" in migration.read_text().lower()
        for command in UPGRADE[3:5]:
            invoke(arguments(command, based), app, environment)
        invoke([based, "migrate", "apply", "--database-url", "local.db"], app, environment)
        assert records(app, "title") == before, "renaming changed existing data or relation IDs"
        invoke([based, "gen", "all", "--check"], app, environment)
        identity(cargo(app, target, environment, "run"))
        invoke(["cargo", "fmt", "--check"], app, environment)
        cargo(app, target, environment, "clippy")
        verify_formatting(based, app, environment)
        verify_failures(based, app, environment)
    print("Embedded tutorial rename preserved rows, ownership, and parent relations; regenerated typed calls passed")


if __name__ == "__main__":
    main()
