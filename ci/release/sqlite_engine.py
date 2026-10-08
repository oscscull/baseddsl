"""Prove the extracted CLI links a WAL-fixed engine by importing existing smoke metadata."""
import json
import subprocess


def verify(based, root, database):
    probe = (root / "import-probe").resolve()
    probe.mkdir()
    (probe / "models").mkdir()
    (probe / "based.toml").write_text('dialect = "sqlite"\nroot = "models"\n')
    before = database.read_bytes()
    result = subprocess.run([str(based), "import", "--database-url", str(database),
                             "--table", "main.item", "--json"], cwd=probe, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert result.returncode == 0, (result.stdout, result.stderr)
    report = json.loads(result.stdout)
    assert report["status"] == "imported", report
    version = report["catalog"]["source"]["server_version"]
    assert tuple(int(part) for part in version.split(".")) >= (3, 51, 3), version
    assert database.read_bytes() == before, "native import changed the smoke database"
    assert not (probe / "migrations").exists()
    print(f"extracted CLI SQLite {version}: WAL-reset fix present; metadata import unchanged")
