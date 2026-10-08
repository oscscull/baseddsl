"""Keep the executable onboarding steps and published command blocks in agreement."""
import re
import shlex

EMBEDDED_START = ("based init --mode embedded", "based migrate apply --database-url local.db", "cargo run")
UPGRADE = ("based check", "based gen all", "based migrate gen . rename_item_title",
           "based migrate verify", "based migrate render . --number 2", "based gen all --check")
STANDALONE_START = ("based init --mode standalone", "python3 tutorial/setup.py",
                    "based check", "based gen all", "based migrate apply --database-url local.db",
                    "python3 tutorial/run.py")


def documented(path):
    return [line.strip() for block in re.findall(r"```sh\n(.*?)```", path.read_text(), re.S)
            for line in block.splitlines() if line.strip()]


def verify(root):
    for name, commands in (("embedded", EMBEDDED_START + UPGRADE),
                           ("standalone", STANDALONE_START + UPGRADE)):
        lines = documented(root / f"docs/{name}-tutorial.md")
        for command in commands:
            assert command in lines, f"{name} tutorial command drift: {command}"
    guard = (root / "examples/standalone-tutorial/guarded-rename.bsl").read_text().strip()
    for path in (root / "docs/standalone-tutorial.md", root / "docs/recipes/transitions.md"):
        assert guard in path.read_text(), f"guarded write snippet drift: {path}"


def arguments(command, based):
    parts = shlex.split(command)
    if parts[0] == "based":
        parts[0] = str(based)
    return parts
