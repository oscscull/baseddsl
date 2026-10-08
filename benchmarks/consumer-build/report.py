#!/usr/bin/env python3
"""Summarize repeated build costs; interpretation and adoption remain an owner decision."""
import argparse
import json
from pathlib import Path
import statistics


def median(samples, variant, step, field="total_seconds"):
    return statistics.median(row[field] for row in samples if row["variant"] == variant and row["step"] == step)


def table(samples):
    print("| Build scenario | OUT_DIR median (range), s | Explicit median (range), s | Added seconds | Relative change | Direct SQLx median, s |")
    print("|---|---:|---:|---:|---:|---:|")
    for step in ["cold", "unchanged", "rust_edit", "bsl_edit", "bsl_add", "bsl_delete"]:
        values = []
        for variant in ["out_dir", "explicit"]:
            timings = [row["total_seconds"] for row in samples if row["variant"] == variant and row["step"] == step]
            values.append(f"{statistics.median(timings):.3f} ({min(timings):.3f}–{max(timings):.3f})")
        auto = median(samples, "out_dir", step)
        explicit = median(samples, "explicit", step)
        direct = f"{median(samples, 'sqlx', step):.3f}" if step in ["cold", "unchanged", "rust_edit"] else "n/a"
        print(f"| {step} | {values[0]} | {values[1]} | {auto - explicit:+.3f} | {(auto / explicit - 1) * 100:+.1f}% | {direct} |")


def regeneration(samples):
    print("\n| Explicit scenario | Generation median, s | Cargo median, s |")
    print("|---|---:|---:|")
    for step in ["cold", "bsl_edit", "bsl_add", "bsl_delete"]:
        print(f"| {step} | {median(samples, 'explicit', step, 'regeneration_seconds'):.3f} | {median(samples, 'explicit', step, 'build_seconds'):.3f} |")


def artifacts(samples):
    print("\n| Variant | Cold Cargo units | Target unique logical bytes | Application bytes |")
    print("|---|---:|---:|---:|")
    for variant in ["out_dir", "explicit", "sqlx"]:
        rows = [row for row in samples if row["variant"] == variant and row["step"] == "cold"]
        sizes = rows[0]["sizes"]
        print(f"| {variant} | {len(rows[0]['units'])} | {sizes['target_unique_logical_bytes']} | {sizes['application_bytes']} |")
    print("\nOUT_DIR compiler crate variants (first trial):")
    cold = next(row for row in samples if row["variant"] == "out_dir" and row["step"] == "cold")
    for unit in cold["units"]:
        if unit["target"].startswith("based_"):
            identity = unit["package"]
            if "@" not in identity:
                identity = f"{unit['target'].replace('_', '-')}@{identity}"
            print(f"- {identity} {unit['kind']} features={unit['features']} debuginfo={unit['profile']['debuginfo']}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    options = parser.parse_args()
    samples = json.loads(options.input.read_text())["samples"]
    table(samples)
    regeneration(samples)
    artifacts(samples)


if __name__ == "__main__":
    main()
