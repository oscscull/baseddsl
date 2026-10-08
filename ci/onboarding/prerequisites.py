"""Report missing external-consumer prerequisites before creating a project."""
import os
from pathlib import Path
import shutil


def require_cli(based):
    if not Path(based).is_file() or not os.access(based, os.X_OK):
        raise RuntimeError("Missing executable candidate CLI; pass --based /absolute/path/to/extracted/based")


def require(based, path=None):
    require_cli(based)
    for tool in ("cargo", "rustc", "git", "rustfmt"):
        if shutil.which(tool, path=path) is None:
            raise RuntimeError(f"Missing {tool}; install Rust 1.94+, Git, and rustup component add rustfmt clippy")


def verify_missing(based, empty_path):
    try:
        require(based, str(empty_path))
    except RuntimeError as error:
        assert "Missing cargo" in str(error) and "Rust 1.94" in str(error), error
    else:
        raise AssertionError("missing Cargo passed prerequisite checks")
