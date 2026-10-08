"""Assemble native binaries and source/upgrade metadata into one flat archive."""
import json
from pathlib import Path
import shutil
import tarfile
import tempfile
import zipfile

from metadata import ROOT, binary_identity


def package(binary_dir, output, target, metadata):
    extension = ".exe" if "windows" in target else ""
    binaries = [binary_dir / f"{name}{extension}" for name in ["based", "based-lsp"]]
    identities = {path.name: binary_identity(path, metadata) for path in binaries}
    suffix = ".zip" if extension else ".tar.gz"
    output.mkdir(parents=True, exist_ok=True)
    artifact = output / f"based-{metadata['version']}-{target}{suffix}"
    assert not artifact.exists(), f"archive already exists: {artifact}"
    with tempfile.TemporaryDirectory(prefix="based-release-payload-") as scratch:
        payload = Path(scratch)
        for binary in binaries:
            shutil.copy2(binary, payload / binary.name)
        shutil.copy2(ROOT / "LICENSE", payload / "LICENSE")
        shutil.copy2(ROOT / "docs/release-notes.md", payload / "RELEASE-NOTES.md")
        manifest = dict(metadata, target=target, binaries=identities)
        (payload / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        (payload / "SOURCE.txt").write_text(f"https://github.com/oscscull/baseddsl/tree/{metadata['commit']}\n")
        if extension:
            with zipfile.ZipFile(artifact, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for path in sorted(payload.iterdir()):
                    archive.write(path, path.name)
        else:
            with tarfile.open(artifact, "w:gz") as archive:
                for path in sorted(payload.iterdir()):
                    archive.add(path, arcname=path.name)
    return artifact


def extract(archive, destination):
    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as source:
            assert all(Path(name).name == name for name in source.namelist())
            source.extractall(destination)
        return
    with tarfile.open(archive) as source:
        assert all(Path(member.name).name == member.name for member in source.getmembers())
        assert all(member.isfile() for member in source.getmembers())
        source.extractall(destination, filter="data")
