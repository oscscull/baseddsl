"""Execute extracted native artifacts against an isolated SQLite project."""
import json
from pathlib import Path
import subprocess
import tempfile

from archive import extract
from metadata import binary_identity


def invoke(binary, app, *args):
    result = subprocess.run([str(binary), *args], cwd=app, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert result.returncode == 0, (args, result.stdout, result.stderr)
    return result


def verify(artifact):
    with tempfile.TemporaryDirectory(prefix="based-release-smoke-") as scratch:
        root = Path(scratch)
        installed = root / "installed"
        installed.mkdir()
        extract(artifact, installed)
        metadata = json.loads((installed / "manifest.json").read_text())
        suffix = ".exe" if "windows" in metadata["target"] else ""
        based = installed / f"based{suffix}"
        lsp = installed / f"based-lsp{suffix}"
        binary_identity(based, metadata)
        binary_identity(lsp, metadata)
        invoke(lsp, root, "--help")
        app = root / "project"
        app.mkdir()
        (app / "based.toml").write_text('dialect = "sqlite"\nroot = "schema"\n[generate]\nclient = "generated/client.rs"\n')
        (app / "schema").mkdir()
        (app / "schema/item.bsl").write_text("Item { id: Id, name: text }\nquery items() -> Item[] { list Item order (id); }\n")
        invoke(based, app, "check")
        invoke(based, app, "gen", "all")
        invoke(based, app, "gen", "all", "--check")
        invoke(based, app, "migrate", "gen")
        invoke(based, app, "migrate", "verify")
        database = str(app / "local.db")
        invoke(based, app, "migrate", "apply", "--database-url", database)
        status = invoke(based, app, "migrate", "status", "--database-url", database)
        assert "pending" not in status.stdout.lower() or "0 pending" in status.stdout.lower(), status.stdout
    print(f"release smoke passed: {artifact.name}")
