"""Ensure a preinstalled CLI cannot satisfy an external onboarding command."""
import os


def forbid_path_cli(directory, environment):
    directory.mkdir()
    sentinel = directory / "based"
    sentinel.write_text("#!/bin/sh\necho 'Unexpected preinstalled based lookup; use the candidate path' >&2\nexit 97\n")
    sentinel.chmod(0o700)
    return dict(environment, PATH=str(directory) + os.pathsep + environment.get("PATH", ""))
