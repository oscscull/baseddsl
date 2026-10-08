#!/usr/bin/env python3
"""Replay a clean external standalone lesson using only a supplied native CLI and Python."""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "ci/standalone_tutorial"))
import boundary
import headers
from evolution import evolve
import replay


def invoke(based, app, *args):
    subprocess.run([str(based), *args], cwd=app, check=True)


def ports():
    with socket.socket() as edge, socket.socket() as backend, socket.socket() as callback:
        sockets = {"edge": edge, "backend": backend, "callback": callback}
        for listener in sockets.values():
            listener.bind(("127.0.0.1", 0))
        return {name: listener.getsockname()[1] for name, listener in sockets.items()}


@contextmanager
def directory(path):
    previous = Path.cwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(previous)


def lesson(based, app, settings):
    import backend
    from listeners import serving
    from requests import success
    with directory(app), serving(settings) as servers:
        process = backend.start(settings, str(based))
        try:
            headers.verify(settings)
            item = boundary.verify(settings)
            payload, first = replay.rename(settings, item)
        finally:
            backend.stop(process)
        process = backend.start(settings, str(based))
        try:
            replay.restarted(settings, payload, first)
        finally:
            backend.stop(process)
        evolve(based, app)
        process = backend.start(settings, str(based))
        try:
            assert success(settings, "/q/item_by_id", {"id": item["id"]}) == first
            assert success(settings, "/m/rename_item", payload, key="lesson-rename-1") == first
            servers["callback"].shutdown()
            servers["callback"].server_close()
            replay.unavailable(settings, payload)
        finally:
            backend.stop(process)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    parser.add_argument("--archive", type=Path, help="exercise the extracted release lesson, not source files")
    args = parser.parse_args()
    based = args.based.resolve()
    with tempfile.TemporaryDirectory(prefix="based-standalone-lesson-") as scratch:
        app = Path(scratch) / "app"
        invoke(based, ROOT, "init", str(app), "--mode", "standalone")
        if args.archive:
            with zipfile.ZipFile(args.archive) as archive:
                archive.extractall(app)
        else:
            shutil.copytree(ROOT / "examples/standalone-tutorial", app / "tutorial",
                            ignore=shutil.ignore_patterns("__pycache__", ".env.json"))
        chosen = ports()
        subprocess.run([sys.executable, str(app / "tutorial/setup.py"),
                        *[arg for name, port in chosen.items() for arg in (f"--{name}-port", str(port))]],
                       cwd=app, check=True)
        invoke(based, app, "gen", "all")
        invoke(based, app, "migrate", "apply", "--database-url", "local.db")
        sys.path.insert(0, str(app / "tutorial"))
        settings = json.loads((app / "tutorial/.env.json").read_text())
        lesson(based, app, settings)
    print("Standalone authentication, scoped relations, durable restart replay, guard failures, and schema evolution passed")


if __name__ == "__main__":
    main()
