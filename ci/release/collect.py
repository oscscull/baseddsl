#!/usr/bin/env python3
"""Validate and checksum collected native assets before a separate draft-release step."""
import argparse
import json
import tarfile
import zipfile
from pathlib import Path

import checksums
import metadata
from extension import verify as verify_extension



def manifest(path):
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(path) as archive:
            return json.loads(archive.read("manifest.json"))
    with tarfile.open(path) as archive:
        file = archive.extractfile("manifest.json")
        assert file is not None
        return json.load(file)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    parser.add_argument("--expected-count", type=int, default=5)
    options = parser.parse_args()
    archives = sorted(list(options.directory.glob("*.tar.gz")) + list(options.directory.glob("*.zip")))
    assert len(archives) == options.expected_count, [path.name for path in archives]
    source = metadata.source()
    extensions = list(options.directory.glob("*.vsix"))
    assert len(extensions) == 1, extensions
    verify_extension(extensions[0], source["version"])
    targets = []
    for archive in archives:
        record = manifest(archive)
        assert record["version"] == source["version"]
        assert record["commit"] == source["commit"]
        assert not record["dirty"]
        targets.append(record["target"])
    assert len(set(targets)) == len(targets), targets
    checksums.write(options.directory)
    checksums.verify(options.directory)
    print(f"collected {len(archives)} native archives with verified checksums")


if __name__ == "__main__":
    main()
