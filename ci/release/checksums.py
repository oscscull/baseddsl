"""Create and verify portable SHA-256 asset inventories."""
import hashlib
from pathlib import Path


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def write(directory):
    assets = sorted(path for path in directory.iterdir() if path.is_file() and path.name != "SHA256SUMS")
    inventory = directory / "SHA256SUMS"
    inventory.write_text("".join(f"{digest(path)}  {path.name}\n" for path in assets))
    return inventory


def verify(directory):
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        expected, name = line.split("  ", 1)
        assert Path(name).name == name, name
        assert digest(directory / name) == expected, name
