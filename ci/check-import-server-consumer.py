#!/usr/bin/env python3
"""Read an independently provisioned server fixture; its caller verifies preservation."""
import argparse
import os
from pathlib import Path
import sys

from initializer import ROOT
sys.path.insert(0, str(ROOT / "ci/import_workflow"))
from consumer import verify
from urllib.parse import urlsplit, urlunsplit


def connection(source, dialect, role, password):
    """Keep the supplied URL's TLS query intact; SQLx URL serialization alters CA paths."""
    parts = urlsplit(source)
    authority = parts.netloc.rsplit("@", 1)[-1]
    path = "/based_import_fixture" if dialect == "mariadb" else parts.path
    return urlunsplit((parts.scheme, f"based_import_{role}:{password}@{authority}", path,
                       parts.query, parts.fragment))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--dialect", choices=("mariadb", "postgres"), required=True)
    options = parser.parse_args()
    source = os.environ[{"mariadb": "TEST_MARIADB_URL", "postgres": "TEST_POSTGRES_URL"}[options.dialect]]
    environment = dict(os.environ,
                       BASED_DATABASE_URL=connection(source, options.dialect, "metadata", "fixture_metadata_only"),
                       DATABASE_URL=connection(source, options.dialect, "consumer", "fixture_select_only"))
    report = verify(options.based.resolve(), options.project.resolve(), options.dialect,
                    "based_import_fixture", environment)
    print(f"{options.dialect} {report['catalog']['source']['server_version']}: fresh imported typed read passed")


if __name__ == "__main__":
    main()
