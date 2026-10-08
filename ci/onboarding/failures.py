"""Verify actionable CLI failures in the fresh external consumer without losing its files."""
from initializer import invoke


def verify(based, app, environment):
    manifest = app / "based.toml"
    before = manifest.read_bytes()
    manifest.write_text('dialect = "not-a-database"\n')
    try:
        error = invoke([based, "check"], app, environment, success=False)
        assert "dialect" in error and "error" in error, error
    finally:
        manifest.write_bytes(before)
    artifact = app / "generated/client.rs"
    generated = artifact.read_bytes()
    artifact.write_text("// User-owned output: preserve me\n")
    try:
        error = invoke([based, "gen", "all"], app, environment, success=False)
        assert "owned" in error and "--force" in error, error
        assert artifact.read_text() == "// User-owned output: preserve me\n"
    finally:
        artifact.write_bytes(generated)
    missing = app / "absent-directory/database.db"
    error = invoke([based, "migrate", "status", "--database-url", missing], app,
                   environment, success=False)
    assert "connect" in error or "open" in error, error
    assert not missing.exists()
