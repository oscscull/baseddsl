"""Load operator-owned local identities and private listener addresses."""
import json
from pathlib import Path

OWNER = "00000000-0000-4000-8000-000000000001"
OTHER_OWNER = "00000000-0000-4000-8000-000000000002"


def load():
    return json.loads((Path(__file__).resolve().parent / ".env.json").read_text())


def address(settings, listener):
    return ("127.0.0.1", settings["ports"][listener])
