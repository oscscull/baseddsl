#!/usr/bin/env python3
"""Verify the optional build helper in an isolated, fresh Cargo consumer."""
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "ci/fixtures/cargo-generation"
CARGO = os.environ.get("CARGO", "cargo")


def command(app, arguments, env, success=True):
    result = subprocess.run([CARGO, *arguments], cwd=app, env=env, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if (result.returncode == 0) != success:
        raise AssertionError(f"cargo {' '.join(arguments)}:\n{result.stdout}\n{result.stderr}")
    return result


def consumer(directory):
    app = directory / "app"
    shutil.copytree(FIXTURE, app, ignore=shutil.ignore_patterns("target", "Cargo.lock"))
    manifest = app / "Cargo.toml"
    text = manifest.read_text()
    for name in ["based-build", "based-runtime"]:
        text = text.replace(f"../../../crates/{name}", (ROOT / "crates" / name).as_posix())
    manifest.write_text(text)
    blocked = directory / "blocked-tools"
    blocked.mkdir()
    for tool in ["based", "rustfmt"]:
        path = blocked / tool
        path.write_text("#!/bin/sh\necho unexpected external generation tool >&2\nexit 97\n")
        path.chmod(0o755)
    env = dict(os.environ, CARGO_TARGET_DIR=str(directory / "target"),
               PATH=f"{blocked}{os.pathsep}{os.environ['PATH']}")
    command(app, ["generate-lockfile"], env)
    return app, env


def files(env):
    clients = list(Path(env["CARGO_TARGET_DIR"]).glob("debug/build/cargo-generation-consumer-*/out/client.rs"))
    assert len(clients) == 1, clients
    return clients[0], clients[0].with_name("verification.txt")


def no_compiler(app, env, client, record):
    before = (client.stat().st_mtime_ns, record.stat().st_mtime_ns)
    command(app, ["build", "--locked"], env)
    assert client.stat().st_mtime_ns == before[0]
    if record.stat().st_mtime_ns != before[1]:
        assert record.read_text() == "checked=false changed=false\n", record.read_text()


def schema_lifecycle(app, env, client, record):
    original = client.read_bytes()
    timestamp = client.stat().st_mtime_ns
    source = app / "schema/item.bsl"
    schema = source.read_text()
    source.write_text(schema + "\n# Non-semantic edit still checks the schema.\n")
    command(app, ["build", "--locked"], env)
    assert record.read_text() == "checked=true changed=false\n"
    assert client.stat().st_mtime_ns == timestamp
    extra = app / "schema/nested/extra.bsl"
    extra.parent.mkdir()
    extra.write_text("query other_items() -> ItemRow[] { list Item; }\n")
    command(app, ["build", "--locked"], env)
    assert record.read_text() == "checked=true changed=true\n"
    assert b"pub async fn other_items" in client.read_bytes()
    extra.unlink()
    extra.parent.rmdir()
    command(app, ["build", "--locked"], env)
    assert record.read_text() == "checked=true changed=true\n"
    assert client.read_bytes() == original
    timestamp = client.stat().st_mtime_ns
    source.write_text("Item { id: serial, name:\n")
    failure = command(app, ["build", "--locked"], env, success=False)
    assert re.search(r"item\.bsl:\d+:\d+: E\d+:", failure.stderr), failure.stderr
    assert "check failed:" in failure.stderr
    assert client.read_bytes() == original
    assert client.stat().st_mtime_ns == timestamp
    source.write_text(schema)
    command(app, ["build", "--locked"], env)
    no_compiler(app, env, client, record)


def verify():
    tree = subprocess.check_output([CARGO, "tree", "-p", "based-build", "-e", "normal,build",
                                    "--prefix", "none", "--format", "{p}"], cwd=ROOT, text=True)
    packages = {line.split()[0] for line in tree.splitlines() if line.strip()}
    forbidden = {"based-runtime", "sqlx", "sqlx-core", "tokio", "axum", "hyper", "reqwest"}
    assert not packages & forbidden, packages & forbidden
    with tempfile.TemporaryDirectory(prefix="based-cargo-generation-") as directory:
        app, env = consumer(Path(directory))
        command(app, ["run", "--locked"], env)
        client, record = files(env)
        assert record.read_text() == "checked=true changed=true\n"
        no_compiler(app, env, client, record)
        client.unlink()
        command(app, ["build", "--locked"], env)
        assert record.read_text() == "checked=true changed=true\n"
        main = app / "src/main.rs"
        main.write_text(main.read_text() + "\nfn unrelated_rust_edit() {}\n")
        no_compiler(app, env, client, record)
        content, timestamp = client.read_bytes(), client.stat().st_mtime_ns
        fmt_env = dict(env, PATH=os.environ["PATH"])
        command(app, ["fmt", "--all"], fmt_env)
        assert client.read_bytes() == content
        assert client.stat().st_mtime_ns == timestamp
        schema_lifecycle(app, env, client, record)
        # Mode comes from the same manifest option as explicit CLI generation.
        manifest = app / "based.toml"
        embedded = manifest.read_text()
        manifest.write_text(embedded.replace('client_mode = "embedded"', 'client_mode = "wire"'))
        command(app, ["build", "--locked"], env, success=False)
        assert record.read_text() == "checked=true changed=true\n"
        assert b"pub fn embedded(" not in client.read_bytes()
        manifest.write_text(embedded)
        command(app, ["build", "--locked"], env)
        assert b"pub fn embedded(" in client.read_bytes()
        # Shared-root layouts may rerun build.rs after a Rust edit, but not the compiler.
        manifest = app / "based.toml"
        manifest.write_text(manifest.read_text().replace('root = "schema"\n', ""))
        command(app, ["build", "--locked"], env)
        main.write_text(main.read_text() + "\nfn shared_root_rust_edit() {}\n")
        no_compiler(app, env, client, record)
        assert not list(app.rglob("client.rs")), "generated client escaped OUT_DIR"
    print("ci-cargo-generation: fresh client call, input lifecycle, cache, formatter and compiler-only graph passed")


if __name__ == "__main__":
    verify()
