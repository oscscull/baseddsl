#!/usr/bin/env python3
"""Compile fresh CLI-generated clients outside the workspace (expensive CI tier)."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CARGO = os.environ.get("CARGO", "cargo")


def run(args, cwd, env=None):
    subprocess.run(args, cwd=cwd, env=env, check=True)


def consumer_manifest(dialect):
    runtime = ROOT / "crates/based-runtime"
    codegen = ROOT / "crates/based-codegen"
    return f'''[package]
name = "based-consumer-contract"
version = "0.0.0"
edition = "2021"
[workspace]
[features]
sqlite = []
mariadb = []
postgres = []
[dependencies]
based-runtime = {{ path = "{runtime}", features = ["{dialect}", "id-gen"] }}
based-codegen = {{ path = "{codegen}", default-features = false }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
bigdecimal = "0.4"
futures-core = "0.3"
futures-util = {{ version = "0.3", default-features = false, features = ["std"] }}
async-trait = "0.1.92"
sqlx = {{ version = "0.9.0", default-features = false, features = ["runtime-tokio", "{ 'mysql' if dialect == 'mariadb' else dialect }"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
'''


def verify_ids(project, env, dialect):
    shutil.copy(ROOT / "ci/fixtures/generated-consumer/wrong.rs", project / "src/wrong.rs")
    with (project / "Cargo.toml").open("a") as manifest:
        manifest.write('\n[[bin]]\nname = "wrong"\npath = "src/wrong.rs"\n')
    result = subprocess.run([CARGO, "check", "--locked", "--offline", "--features", dialect, "--bin", "wrong"],
                            cwd=project, env=env, capture_output=True, text=True)
    assert result.returncode != 0, "entity IDs were interchangeable"
    assert "mismatched types" in result.stderr and "Owner" in result.stderr and "Item" in result.stderr, result.stderr


def verify(dialect, binary):
    with tempfile.TemporaryDirectory(prefix="based-consumer-") as directory:
        project = Path(directory)
        fixture = ROOT / "ci/fixtures/generated-consumer"
        (project / "src").mkdir()
        (project / "generated").mkdir()
        shutil.copy(fixture / "schema.bsl", project / "schema.bsl")
        shutil.copy(fixture / "main.rs", project / "src/main.rs")
        (project / "based.toml").write_text(f'dialect = "{dialect}"\n')
        (project / "Cargo.toml").write_text(consumer_manifest(dialect))
        run([str(binary), "gen", "client", "--embedded", "-o", "generated/client.rs"], project)
        env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / "target/consumer-contract"))
        url_key = f"TEST_{dialect.upper()}_URL"
        env["DATABASE_URL"] = str(project / "database.db") if dialect == "sqlite" else os.environ[url_key]
        run([CARGO, "run", "--features", dialect, "--bin", "based-consumer-contract"], project, env)
        verify_ids(project, env, dialect)
        print(f"generated-consumer: {dialect} passed (including rejected mixed entity IDs)", flush=True)


def verify_committed(binary):
    for example in ["sqlite-quickstart", "mariadb-quickstart", "postgres-quickstart", "axum-helpdesk"]:
        project = ROOT / "examples" / example
        generated = project / "generated/client.rs"
        result = subprocess.run([str(binary), "gen", "client", "--embedded"], cwd=project,
                                check=True, capture_output=True)
        assert result.stdout == generated.read_bytes(), f"stale client: {generated}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dialect", choices=["sqlite", "mariadb", "postgres"])
    args = parser.parse_args()
    binary = ROOT / "target/debug/based"
    verify_committed(binary)
    verify(args.dialect, binary)


if __name__ == "__main__":
    main()
