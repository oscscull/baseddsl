"""Verify custom consumer formatting never modifies the generated artifact."""
from initializer import invoke


def verify(based, app, environment):
    artifact = app / "generated/client.rs"
    before = artifact.read_bytes()
    invoke(["git", "init", "--initial-branch=onboarding"], app, environment)
    invoke(["git", "add", "generated/client.rs"], app, environment)
    invoke(["git", "-c", "commit.gpgsign=false", "-c", "user.name=Onboarding verification", "-c", "user.email=onboarding@example.invalid",
            "commit", "-m", "Record explicit generated artifact"], app, environment)
    (app / "rustfmt.toml").write_text("max_width = 60\nuse_small_heuristics = \"Off\"\n")
    invoke(["cargo", "fmt"], app, environment)
    invoke(["cargo", "fmt", "--check"], app, environment)
    assert artifact.read_bytes() == before, "Cargo formatting touched the generated client"
    child = app / "src/nested"
    child.mkdir()
    invoke([based, "gen", "all", "--check"], child, environment)
    invoke([based, "gen", "all"], child, environment)
    assert artifact.read_bytes() == before, "subdirectory or formatter config changed generation"
    invoke(["git", "diff", "--exit-code", "HEAD", "--", ":(top)generated/client.rs"], child, environment)
