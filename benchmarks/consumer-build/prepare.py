"""Create matched scratch consumers from a single locked fixture."""
import os
from pathlib import Path
import shutil
import hashlib

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = Path(__file__).resolve().parent / "fixture"
FEATURES = {"out_dir": "cargo-generation", "explicit": "embedded", "sqlx": "direct"}


def consumer(directory, variant, jobs):
    app = directory / "app"
    shutil.copytree(FIXTURE, app, ignore=shutil.ignore_patterns("target"))
    manifest = app / "Cargo.toml"
    text = manifest.read_text()
    for name in ["based-build", "based-runtime"]:
        text = text.replace(f"../../../crates/{name}", (ROOT / "crates" / name).as_posix())
    manifest.write_text(text)
    if variant != "out_dir":
        (app / "build.rs").unlink()
    env = dict(os.environ, CARGO_TARGET_DIR=str(directory / "target"),
               CARGO_BUILD_JOBS=str(jobs), CARGO_NET_OFFLINE="true")
    return app, env


def lock_hash():
    return hashlib.sha256((FIXTURE / "Cargo.lock").read_bytes()).hexdigest()
