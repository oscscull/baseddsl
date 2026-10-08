#!/usr/bin/env python3
"""Verify the documented data-preserving rename in a fresh external consumer."""
import argparse
import os
from pathlib import Path
import sqlite3
import tempfile

from initializer import ROOT, cargo, fresh, identity, invoke
from metadata import git
from source import mirror


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
    with tempfile.TemporaryDirectory(prefix="based-embedded-tutorial-") as scratch:
        root = Path(scratch).resolve()
        source_url = mirror(ROOT, git("rev-parse", "HEAD"), root / "source.git")
        environment = dict(os.environ, CARGO_NET_GIT_FETCH_WITH_CLI="true", GIT_CONFIG_COUNT="1",
                           GIT_CONFIG_KEY_0=f"url.{source_url}.insteadOf",
                           GIT_CONFIG_VALUE_0="https://github.com/oscscull/baseddsl.git")
        environment.pop("DATABASE_URL", None)
        environment.pop("BASED_DATABASE_URL", None)
        app = root / "app"
        target = root / "target"
        fresh(based, app, "embedded", "sqlite", environment)
        invoke([based, "migrate", "apply", "--database-url", "local.db"], app, environment)
        identity(cargo(app, target, environment, "run"))
        before = records(app, "name")
        assert len(before) == 2
        rename_source(app / "schema/item.bsl")
        invoke([based, "gen", "all"], app, environment)
        invoke([based, "migrate", "gen", ".", "rename_item_title"], app, environment)
        assert records(app, "name") == before, "generation silently changed the database"
        migration = app / "migrations/0002_rename_item_title/up.mig"
        assert "rename" in migration.read_text().lower()
        invoke([based, "migrate", "verify"], app, environment)
        invoke([based, "migrate", "apply", "--database-url", "local.db"], app, environment)
        assert records(app, "title") == before, "renaming changed existing data or relation IDs"
        invoke([based, "gen", "all", "--check"], app, environment)
        identity(cargo(app, target, environment, "run"))
        invoke(["cargo", "fmt", "--check"], app, environment)
        cargo(app, target, environment, "clippy")
    print("Embedded tutorial rename preserved rows, ownership, and parent relations; regenerated typed calls passed")


if __name__ == "__main__":
    main()
