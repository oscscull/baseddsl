"""Serialize measurement evidence with compact compiler-unit rows for review."""
import json


def encode(value, level=0, key=None):
    indent = "  " * level
    child_indent = "  " * (level + 1)
    if isinstance(value, dict):
        entries = [f"{child_indent}{json.dumps(name)}: {encode(item, level + 1, name)}"
                   for name, item in value.items()]
        return "{\n" + ",\n".join(entries) + f"\n{indent}" + "}"
    if isinstance(value, list):
        if not value:
            return "[]"
        if key in ["units", "rebuilt_units"]:
            entries = [child_indent + json.dumps(item, separators=(",", ":")) for item in value]
        else:
            entries = [child_indent + encode(item, level + 1) for item in value]
        return "[\n" + ",\n".join(entries) + f"\n{indent}]"
    return json.dumps(value)


def write(path, evidence):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(encode(evidence) + "\n")
