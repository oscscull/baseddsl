#!/usr/bin/env python3
"""Verify benchmark artifacts track current codegen; compile/run via portable Make target."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PROJECT = ROOT / "benchmarks/embedded-runtime"
BASED = Path(os.environ.get("BASED_BIN", str(ROOT / "target/debug/based"))).resolve()


def normalize(text):
    return " ".join(line for line in text.splitlines() if not line.startswith("--")).split()


def check():
    sql = subprocess.check_output([str(BASED), "gen", "sql"], cwd=PROJECT, text=True)
    tables = sql.split("-- ============================== queries")[0]
    assert normalize(tables) == normalize((PROJECT / "schema.sql").read_text()), "schema.sql drift"
    with tempfile.TemporaryDirectory(prefix="based-benchmark-codegen-") as scratch:
        output = Path(scratch) / "client.rs"
        subprocess.run([str(BASED), "gen", "client", "--embedded", "-o", str(output)],
                       cwd=PROJECT, check=True)
        assert output.read_bytes() == (PROJECT / "generated/client.rs").read_bytes(), "client.rs drift"


if __name__ == "__main__":
    check()
