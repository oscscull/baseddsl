"""Validate component metadata and identify the exact native release source."""
import json
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def source(expected_tag=None, allow_dirty=False):
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]
    version = workspace["version"]
    assert re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", version), version
    if expected_tag:
        assert expected_tag == f"v{version}", (expected_tag, version)
    for manifest in (ROOT / "crates").glob("*/Cargo.toml"):
        package = tomllib.loads(manifest.read_text())["package"]
        assert package["version"] == {"workspace": True}, manifest
        assert package["license"] == {"workspace": True}, manifest
        assert package["rust-version"] == {"workspace": True}, manifest
    extension = json.loads((ROOT / "editors/vscode/package.json").read_text())
    extension_lock = json.loads((ROOT / "editors/vscode/package-lock.json").read_text())
    assert extension["version"] == extension_lock["version"] == version
    assert extension_lock["packages"][""]["version"] == version
    dirty = bool(git("status", "--porcelain"))
    assert allow_dirty or not dirty, "release source is dirty; use --allow-dirty only for a local dry run"
    return {"version": version, "commit": git("rev-parse", "HEAD"), "dirty": dirty,
            "license": workspace["license"], "rust_version": workspace["rust-version"],
            "tag": expected_tag}


def binary_identity(binary, metadata):
    result = subprocess.check_output([str(binary), "--version"], text=True).strip()
    assert f" {metadata['version']} (" in result, result
    assert metadata["commit"][:12] in result, (metadata["commit"], result)
    assert metadata["dirty"] or "-dirty" not in result, result
    return result


def native_toolchain():
    version = subprocess.check_output(["rustc", "-Vv"], text=True).strip()
    host = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
    return {"rustc": version, "target": host}
