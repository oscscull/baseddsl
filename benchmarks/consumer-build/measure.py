#!/usr/bin/env python3
"""Repeated matched consumer builds; all source edits and target dirs are scratch-only."""
import argparse
import json
import os
from pathlib import Path
import tempfile

import cargo
import environment
import prepare
import scenario


def sample(directory, variant, trial, options):
    app, env = prepare.consumer(directory, variant, options.jobs)
    steps = ["cold", "unchanged", "rust_edit"]
    if variant != "sqlx":
        steps.extend(["bsl_edit", "bsl_add", "bsl_delete"])
    records = []
    for step in steps:
        before = scenario.evidence(env)
        scenario.apply(app, step)
        regeneration = 0.0
        if variant == "explicit" and step in ["cold", "bsl_edit", "bsl_add", "bsl_delete"]:
            regeneration, _ = cargo.timed([str(options.based), "gen", "client"], app, env)
        seconds, result = cargo.timed([options.cargo, "build", "--locked", "--offline",
                                       "--no-default-features", "--features", prepare.FEATURES[variant],
                                       "--message-format=json"], app, env)
        after = scenario.evidence(env)
        invalidation = scenario.verify_invalidation(before, after, step, variant)
        compiled_units = cargo.units(result.stdout, Path(env["CARGO_TARGET_DIR"]))
        record = {"variant": variant, "trial": trial, "step": step, "build_seconds": seconds,
                  "regeneration_seconds": regeneration, "total_seconds": seconds + regeneration,
                  "invalidation": invalidation,
                  "rebuilt_units": [unit for unit in compiled_units if not unit["fresh"]]}
        if step == "cold":
            if variant != "sqlx":
                scenario.verify_client(app, env, variant)
            record["units"] = compiled_units
            record["sizes"] = cargo.sizes(Path(env["CARGO_TARGET_DIR"]), app)
        records.append(record)
        # Run the exact built executable untimed: every variant must return the same row.
        cargo.timed([str(Path(env["CARGO_TARGET_DIR"]) / "debug/consumer-build-cost")], app, env)
        print(f"trial {trial} {variant} {step}: {record['total_seconds']:.3f}s ({invalidation})", flush=True)
    return records



def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    parser.add_argument("--cargo", default=os.environ.get("CARGO", "cargo"))
    parser.add_argument("--jobs", type=int, default=14)
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--output", type=Path, required=True)
    options = parser.parse_args()
    if options.trials < 1:
        parser.error("--trials must be positive")
    if options.jobs < 1:
        parser.error("--jobs must be positive")
    options.based = options.based.resolve()
    result = {"environment": environment.capture(options), "samples": []}
    with tempfile.TemporaryDirectory(prefix="based-build-cost-") as scratch:
        for trial in range(1, options.trials + 1):
            variants = ["out_dir", "explicit", "sqlx"]
            if trial % 2 == 0:
                variants.reverse()
            for variant in variants:
                directory = Path(scratch) / f"{trial}-{variant}"
                directory.mkdir()
                result["samples"].extend(sample(directory, variant, trial, options))
                options.output.parent.mkdir(parents=True, exist_ok=True)
                options.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
