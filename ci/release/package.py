#!/usr/bin/env python3
"""Package and smoke native artifacts; never create tags or publish a release."""
import argparse
from pathlib import Path

import archive
import checksums
import metadata
import smoke


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--tag")
    parser.add_argument("--allow-dirty", action="store_true")
    options = parser.parse_args()
    source = metadata.source(options.tag, options.allow_dirty)
    toolchain = metadata.native_toolchain()
    assert options.target == toolchain["target"], "package and smoke each target on its native runner"
    source["toolchain"] = toolchain
    artifact = archive.package(options.binary_dir.resolve(), options.output.resolve(), options.target, source)
    smoke.verify(artifact)
    checksums.write(options.output)
    checksums.verify(options.output)
    print(artifact)


if __name__ == "__main__":
    main()
