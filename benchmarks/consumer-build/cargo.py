"""Time whole Cargo commands and retain compiler unit/feature evidence."""
import json
from pathlib import Path
import subprocess
import time
from urllib.parse import unquote


def timed(arguments, app, env):
    start = time.perf_counter()
    result = subprocess.run(arguments, cwd=app, env=env, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    seconds = time.perf_counter() - start
    if result.returncode:
        raise RuntimeError(f"{' '.join(arguments)}:\n{result.stdout}\n{result.stderr}")
    return seconds, result


def messages(stdout):
    for line in stdout.splitlines():
        try:
            yield json.loads(line)
        except json.JSONDecodeError:
            continue


def units(stdout, target):
    result = []
    for message in messages(stdout):
        if message.get("reason") != "compiler-artifact":
            continue
        outputs = []
        for name in message["filenames"]:
            path = Path(name)
            outputs.append({"path": path.relative_to(target).as_posix(), "bytes": path.stat().st_size})
        result.append({"package": package_name(message["package_id"]),
                       "target": message["target"]["name"], "kind": message["target"]["kind"],
                       "features": message["features"], "profile": message["profile"],
                       "outputs": outputs, "fresh": message["fresh"]})
    return result


def sizes(target, app):
    seen = set()
    logical = 0
    for path in target.rglob("*"):
        if not path.is_file():
            continue
        stat = path.stat()
        key = (stat.st_dev, stat.st_ino)
        if key in seen:
            continue
        seen.add(key)
        logical += stat.st_size
    executable = target / "debug/consumer-build-cost"
    clients = list(target.glob("debug/build/consumer-build-cost-*/out/client.rs"))
    explicit = app / "generated/client.rs"
    return {"target_unique_logical_bytes": logical, "application_bytes": executable.stat().st_size,
            "out_dir_client_bytes": sum(path.stat().st_size for path in clients),
            "explicit_client_bytes": explicit.stat().st_size}


def package_name(identity):
    source, _, fragment = identity.rpartition("#")
    if "@" in fragment:
        return fragment
    # Cargo omits the name in a path ID when it matches the source directory.
    return f"{unquote(source.rstrip('/').rsplit('/', 1)[-1])}@{fragment}"
