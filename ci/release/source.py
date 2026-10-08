"""Stage an exact committed Git source for isolated consumer verification."""
from pathlib import Path
import subprocess


def mirror(root: Path, commit: str, destination: Path):
    subprocess.run(["git", "init", "--bare", "--initial-branch=release-source", str(destination)], check=True)
    subprocess.run(["git", "--git-dir", str(destination), "fetch", "--no-tags", str(root), commit], check=True)
    subprocess.run(["git", "--git-dir", str(destination), "update-ref", "refs/heads/release-source", commit], check=True)
    return destination.as_uri()
