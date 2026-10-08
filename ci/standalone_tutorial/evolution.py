"""Verify explicit schema regeneration preserves persisted values and relations."""
import sqlite3
import subprocess


def invoke(based, app, *args):
    subprocess.run([str(based), *args], cwd=app, check=True)


def evolve(based, app):
    with sqlite3.connect(app / "local.db") as database:
        before = database.execute("SELECT id, name, owner, parent_id FROM item ORDER BY id").fetchall()
    schema = app / "schema/item.bsl"
    schema.write_text(schema.read_text().replace("  name: text", '  title: text @was("name")')
                      .replace("  name\n", "  name = title\n")
                      .replace("parent { id, name }", "parent { id, name = title }")
                      .replace("name = $name", "title = $name"))
    guarded = app / "schema/guarded-rename.bsl"
    guarded.write_text(guarded.read_text().replace("name = $expected_name", "title = $expected_name")
                       .replace("{ name = $name }", "{ title = $name }"))
    invoke(based, app, "gen", "all")
    invoke(based, app, "migrate", "gen", ".", "rename_item_title")
    invoke(based, app, "migrate", "verify")
    invoke(based, app, "migrate", "render", ".", "--number", "2")
    invoke(based, app, "migrate", "apply", "--database-url", "local.db")
    invoke(based, app, "gen", "all", "--check")
    with sqlite3.connect(app / "local.db") as database:
        assert database.execute("SELECT id, title, owner, parent_id FROM item ORDER BY id").fetchall() == before
