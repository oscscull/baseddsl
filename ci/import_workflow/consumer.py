"""Run imported hand-authored models through a fresh public-Git Rust consumer."""
import json
from pathlib import Path
import tempfile

from initializer import ROOT, cargo, invoke
from metadata import git
from source import mirror


from rust import READS, MAIN, backend, manifest


def verify(based, app, dialect, namespace, environment):
    """The caller owns independent fixture creation and unchanged-DB assertions."""
    args = [based, "import", "--table", f"{namespace}.legacy_account", "--table", f"{namespace}.legacy_entry", "--json"]
    report = json.loads(invoke(args, app, environment))
    assert report["status"] == "imported" and len(report["written"]) == 2, report
    assert not (app / "migrations").exists()
    (app / "models/reads.bsl").write_text(READS)
    (app / "src").mkdir(exist_ok=True)
    (app / "src/main.rs").write_text(MAIN)
    (app / "src/database.rs").write_text(backend(dialect))
    (app / "Cargo.toml").write_text(manifest(dialect, git("rev-parse", "HEAD")))
    invoke([based, "check"], app, environment)
    invoke([based, "gen", "client", "--embedded"], app, environment)
    invoke(["cargo", "fmt"], app, environment)
    with tempfile.TemporaryDirectory(prefix="based-import-git-") as temporary:
        source = mirror(ROOT, git("rev-parse", "HEAD"), Path(temporary).resolve() / "source.git")
        consumer_env = dict(environment, CARGO_NET_GIT_FETCH_WITH_CLI="true", GIT_CONFIG_COUNT="1",
                            GIT_CONFIG_KEY_0=f"url.{source}.insteadOf", GIT_CONFIG_VALUE_0="https://github.com/oscscull/baseddsl.git")
        target = Path(temporary) / "target"
        output = cargo(app, target, consumer_env, "run")
        line = next(line for line in output.splitlines() if line.startswith("imported read: "))
        rows = json.loads(line.removeprefix("imported read: "))
        assert rows == [{"heading": "retained original row", "parent_code": {"label": "retained account"}}], rows
        cargo(app, target, consumer_env, "clippy")
        assert f'#{git("rev-parse", "HEAD")}' in (app / "Cargo.lock").read_text()
    assert not (app / "build.rs").exists() and not (app / "migrations").exists()
    return report
