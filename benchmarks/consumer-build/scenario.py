"""Apply one controlled edit and verify whether the compiler actually ran."""
from pathlib import Path
import prepare


def apply(app, step):
    if step == "rust_edit":
        path = app / "src/main.rs"
        path.write_text(path.read_text().replace("black_box(0)", "black_box(1)"))
    if step == "bsl_edit":
        path = app / "schema/item.bsl"
        path.write_text(path.read_text().replace("order (id)", "order (name)"))
    if step == "bsl_add":
        path = app / "schema/nested/extra.bsl"
        path.parent.mkdir()
        path.write_text("query other_items() -> ItemRow[] { list Item order (id); }\n")
    if step == "bsl_delete":
        path = app / "schema/nested/extra.bsl"
        path.unlink()
        path.parent.rmdir()


def evidence(env):
    files = list(Path(env["CARGO_TARGET_DIR"]).glob("debug/build/consumer-build-cost-*/out/verification.txt"))
    if not files:
        return None
    assert len(files) == 1, files
    record = files[0]
    return {"mtime_ns": record.stat().st_mtime_ns, "record": record.read_text()}


def verify_invalidation(before, after, step, variant):
    if variant != "out_dir":
        assert after is None
        return "no build helper"
    assert after is not None
    if step in ["unchanged", "rust_edit"]:
        if before == after:
            return "build script not rerun; BSL compiler not invoked"
        assert after["record"] == "checked=false changed=false\n", (step, before, after)
        return "build script cache hit; BSL compiler and client generation not invoked"
    assert after["record"].startswith("checked=true"), (step, after)
    assert before is None or before["mtime_ns"] != after["mtime_ns"]
    return "BSL compiler invoked"


def payload(text):
    return "\n".join(line for line in text.splitlines() if not line.startswith("// Regenerate "))


def verify_client(app, env, variant):
    client = app / "generated/client.rs"
    if variant == "out_dir":
        clients = list(Path(env["CARGO_TARGET_DIR"]).glob("debug/build/consumer-build-cost-*/out/client.rs"))
        assert len(clients) == 1
        client = clients[0]
    assert payload(client.read_text()) == payload((prepare.FIXTURE / "generated/client.rs").read_text())
