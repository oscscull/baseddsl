#!/usr/bin/env python3
"""Validate and stage the matching VSIX alongside native release artifacts."""
import argparse
import json
from pathlib import Path
import shutil
import zipfile

from metadata import ROOT


def verify(artifact, version):
    with zipfile.ZipFile(artifact) as archive:
        manifest = json.loads(archive.read("extension/package.json"))
        assert manifest["version"] == version
        assert manifest["license"] == "AGPL-3.0-only"
        assert "extension/out/extension.js" in archive.namelist()
        assert "extension/node_modules/vscode-languageclient/lib/node/main.js" in archive.namelist()
        assert not any(name.startswith("extension/test/") for name in archive.namelist())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    parser.add_argument("output", type=Path)
    options = parser.parse_args()
    version = json.loads((ROOT / "editors/vscode/package.json").read_text())["version"]
    artifact = options.directory / f"based-vscode-{version}.vsix"
    verify(artifact, version)
    options.output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(artifact, options.output / artifact.name)
    print(f"validated matching VSIX: {artifact.name}")


if __name__ == "__main__":
    main()
