#!/usr/bin/env python3
"""Package the Python-only standalone lesson with matching source metadata."""
import argparse
import json
from pathlib import Path
import zipfile

import metadata


def verify(artifact, source):
    with zipfile.ZipFile(artifact) as archive:
        record = json.loads(archive.read("tutorial/manifest.json"))
        assert record == source, (record, source)
        assert all(name.startswith("tutorial/") and ".." not in Path(name).parts for name in archive.namelist())
        assert not any(".env.json" in name or "__pycache__" in name for name in archive.namelist())
        assert {"tutorial/run.py", "tutorial/setup.py", "tutorial/guarded-rename.bsl", "tutorial/LICENSE"}.issubset(archive.namelist())


def package(output, source):
    output.mkdir(parents=True, exist_ok=True)
    artifact = output / f'based-standalone-tutorial-{source["version"]}.zip'
    with zipfile.ZipFile(artifact, "x", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted((metadata.ROOT / "examples/standalone-tutorial").iterdir()):
            if path.is_file() and path.name != ".env.json":
                archive.write(path, f"tutorial/{path.name}")
        archive.write(metadata.ROOT / "LICENSE", "tutorial/LICENSE")
        archive.writestr("tutorial/manifest.json", json.dumps(source, indent=2) + "\n")
    verify(artifact, source)
    return artifact


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--allow-dirty", action="store_true")
    options = parser.parse_args()
    print(package(options.output, metadata.source(allow_dirty=options.allow_dirty)))


if __name__ == "__main__":
    main()
