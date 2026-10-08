"""Provision a fresh lesson's guard mapping and local demonstration credentials."""
import argparse
import json
import os
from pathlib import Path
import secrets

from settings import OWNER, OTHER_OWNER


def credentials(ports):
    return {"ports": ports, "guard_secret": secrets.token_urlsafe(32),
            "callers": {name: {"token": secrets.token_urlsafe(32), "owner": owner, "role": role}
                        for name, owner, role in (("editor", OWNER, "editor"),
                                                  ("viewer", OWNER, "viewer"),
                                                  ("other", OTHER_OWNER, "editor"))}}


def write_new(path, content, mode=0o644):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode), "w") as file:
        file.write(content)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for listener, port in (("edge", 8080), ("backend", 9090), ("callback", 9100)):
        parser.add_argument(f"--{listener}-port", type=int, default=port)
    args = parser.parse_args()
    ports = {listener: getattr(args, f"{listener}_port") for listener in ("edge", "backend", "callback")}
    if len(set(ports.values())) != 3 or any(not 1024 <= port <= 65535 for port in ports.values()):
        parser.error("choose three distinct unprivileged ports")
    lesson = Path(__file__).resolve().parent
    schema = Path("schema/guarded-rename.bsl")
    if not Path("schema/item.bsl").is_file():
        parser.error("run from a fresh based init --mode standalone project")
    files = {schema: (lesson / "guarded-rename.bsl").read_text(),
             Path("guards.toml"): '[guards.caller_can_rename]\n'
             f'endpoint = "http://127.0.0.1:{ports["callback"]}/check"\n'
             'secret_env = "BASED_TUTORIAL_GUARD_SECRET"\ndeadline_ms = 2000\n',
             lesson / ".env.json": json.dumps(credentials(ports), indent=2) + "\n"}
    if any(path.exists() or path.is_symlink() for path in files):
        parser.error("lesson is already configured; retain its files or choose a fresh project")
    for path, content in files.items():
        write_new(path, content, 0o600 if path.name == ".env.json" else 0o644)
    print("Configured local credentials and guard mapping. Run based gen all, then start tutorial/run.py.")


if __name__ == "__main__":
    main()
