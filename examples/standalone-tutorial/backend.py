"""Run and stop Based with durable SQLite replay and the named external callback."""
import os
import subprocess
import time
from http.client import HTTPConnection

from settings import address


def start(settings, based="based"):
    host, port = address(settings, "backend")
    environment = dict(os.environ, BASED_TUTORIAL_GUARD_SECRET=settings["guard_secret"])
    process = subprocess.Popen([based, "serve", "--listen", f"{host}:{port}",
                                "--database-url", "local.db", "--idempotency-store", "database",
                                "--init-idempotency-table", "--guard-config", "guards.toml",
                                "--guard-allow-loopback-http"], env=environment)
    try:
        ready(process, (host, port))
        return process
    except Exception:
        stop(process)
        raise


def ready(process, listener):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Based exited; inspect its startup error")
        connection = HTTPConnection(*listener, timeout=1)
        try:
            connection.request("GET", "/readyz")
            if connection.getresponse().status == 200:
                return
        except OSError:
            pass
        finally:
            connection.close()
        time.sleep(0.1)
    raise RuntimeError("Based did not become ready")


def stop(process):
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
