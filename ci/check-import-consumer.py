#!/usr/bin/env python3
"""Import an independent SQLite fixture and perform a real fresh typed Rust read."""
import argparse
import os
from pathlib import Path
import sqlite3
import sys
import tempfile

from initializer import ROOT
sys.path.insert(0, str(ROOT / "ci/import_workflow"))
from consumer import verify


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    options = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="based-import-sqlite-") as scratch:
        app = Path(scratch).resolve()
        (app / "models").mkdir()
        (app / "based.toml").write_text("dialect = 'sqlite'\nroot = 'models'\n[generate]\nclient = 'src/based_client.rs'\nclient_mode = 'embedded'\n")
        database = app / "original.db"
        with sqlite3.connect(database) as fixture:
            fixture.executescript((ROOT / "crates/based-cli/tests/import_support/fixture.sql").read_text())
        before = database.read_bytes()
        environment = dict(os.environ, BASED_DATABASE_URL=str(database), DATABASE_URL=str(database))
        report = verify(options.based.resolve(), app, "sqlite", "main", environment)
        assert database.read_bytes() == before, "import/typed read changed the original SQLite file"
        with sqlite3.connect(database) as fixture:
            assert fixture.execute("SELECT entry_key, parent_code, heading FROM legacy_entry").fetchall() == [(41, "alpha", "retained original row")]
            assert fixture.execute("SELECT name FROM sqlite_schema WHERE name = '_based_migrations'").fetchall() == []
        print(f"SQLite {report['catalog']['source']['server_version']}: imported models and fresh typed read preserved the original database")


if __name__ == "__main__":
    main()
