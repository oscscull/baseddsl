#!/usr/bin/env python3
"""Exercise benchmark correctness without enforcing wall-clock performance thresholds."""
import argparse
import os
from pathlib import Path
import tempfile
import subprocess

import measure
import prepare


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    parser.add_argument("--cargo", default=os.environ.get("CARGO", "cargo"))
    options = parser.parse_args()
    options.based = options.based.resolve()
    options.jobs = 14
    # Fetch before scratch builds; actual measurements are always offline.
    subprocess.run([options.cargo, "fetch", "--locked", "--manifest-path", str(prepare.FIXTURE / "Cargo.toml")], check=True)
    subprocess.run([options.cargo, "fmt", "--manifest-path", str(prepare.FIXTURE / "Cargo.toml"), "--check"], check=True)
    subprocess.run([str(options.based), "gen", "client", "--check"], cwd=prepare.FIXTURE, check=True)
    with tempfile.TemporaryDirectory(prefix="based-build-cost-verification-") as scratch:
        for variant in ["out_dir", "explicit", "sqlx"]:
            directory = Path(scratch) / variant
            directory.mkdir()
            measure.sample(directory, variant, 1, options)
            env = dict(os.environ, CARGO_TARGET_DIR=str(directory / "target"))
            subprocess.run([options.cargo, "clippy", "--locked", "--offline", "--no-default-features", "--features", prepare.FEATURES[variant], "--", "-D", "warnings"], cwd=directory / "app", env=env, check=True)
    print("consumer-build benchmark: all variants have matching results and verified invalidation")


if __name__ == "__main__":
    main()
