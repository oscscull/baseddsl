#!/usr/bin/env python3
"""Run the published embedded path with the existing external tutorial harness."""
import argparse
from pathlib import Path
import subprocess
import sys

from metadata import ROOT


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--based", type=Path, required=True)
    options = parser.parse_args()
    for script in ("check-embedded-tutorial.py", "check-import-consumer.py"):
        subprocess.run([sys.executable, str(ROOT / "ci" / script),
                        "--based", str(options.based.resolve())], check=True)


if __name__ == "__main__":
    main()
