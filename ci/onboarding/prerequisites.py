"""Report missing external-consumer prerequisites before creating a project."""
import os
from pathlib import Path
import shutil
import subprocess


def require_cli(based):
    if not Path(based).is_file() or not os.access(based, os.X_OK):
        raise RuntimeError("Missing executable candidate CLI; pass --based /absolute/path/to/extracted/based")


def require(based, path=None):
    require_cli(based)
    for tool in ("cargo", "rustc", "git", "rustfmt"):
        if shutil.which(tool, path=path) is None:
            raise RuntimeError(f"Missing {tool}; install Rust 1.94+, Git, and rustup component add rustfmt clippy")
    require_component("fmt", "rustfmt")
    require_component("clippy", "clippy")


def require_component(command, component):
    result = subprocess.run(["cargo", command, "--version"], capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"Missing {component} for the active Rust toolchain; run rustup component add {component}")


def verify_missing(based, empty_path):
    try:
        require(based, str(empty_path))
    except RuntimeError as error:
        assert "Missing cargo" in str(error) and "Rust 1.94" in str(error), error
    else:
        raise AssertionError("missing Cargo passed prerequisite checks")
