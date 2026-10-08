#!/usr/bin/env python3
"""Run a local-only service and demonstrate HTTP create/read, then stop it."""
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.error
import urllib.request


def request(base, name, payload, context):
    data = json.dumps(payload).encode()
    call = urllib.request.Request(f"{base}/{name}", data=data, headers={"Content-Type": "application/json", "X-Based-Context": json.dumps(context)})
    with urllib.request.urlopen(call, timeout=5) as response:
        return json.load(response)


def ready(process, base):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("service exited; inspect its error above")
        try:
            with urllib.request.urlopen(f"{base}/readyz", timeout=1):
                return
        except (urllib.error.URLError, TimeoutError):
            time.sleep(0.1)
    raise RuntimeError("service did not become ready")


def main():
    root = Path(__file__).resolve().parent
    database = os.environ.get("DATABASE_URL", "local.db")
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        address = f"127.0.0.1:{reservation.getsockname()[1]}"
    process = subprocess.Popen([os.environ.get("BASED", "based"), "serve", "--listen", address,
                                "--database-url", database, "--idempotency-store", "memory"], cwd=root)
    try:
        base = f"http://{address}"
        ready(process, base)
        # Local host identity only. A production trusted edge authenticates first
        # and discards caller-supplied context; owner UUIDs are not credentials.
        context = {"owner": "00000000-0000-4000-8000-000000000001"}
        parent = request(base, "m/create_item", {"name": "Parent", "parent": None}, context)
        created = request(base, "m/create_item", {"name": "Hello Based", "parent": parent["id"]}, context)
        rows = request(base, "q/items", {}, context)
        print("created:", json.dumps(created))
        print("read:", json.dumps(rows))
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


if __name__ == "__main__":
    main()
