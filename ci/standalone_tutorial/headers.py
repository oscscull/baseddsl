"""Inspect actual forwarding to prove spoofed shard/context and caller auth never propagate."""
from http.client import HTTPConnection
import json

from requests import success


def verify(settings):
    import forward
    captured = []

    class ObservedConnection(HTTPConnection):
        def request(self, method, path, body=None, headers=None, **kwargs):
            captured.append(dict(headers or {}))
            return super().request(method, path, body, headers or {}, **kwargs)

    previous = forward.HTTPConnection
    forward.HTTPConnection = ObservedConnection
    try:
        success(settings, "/q/items", {}, identity="viewer", spoof=True)
    finally:
        forward.HTTPConnection = previous
    assert len(captured) == 1, captured
    assert set(captured[0]) == {"Content-Type", "X-Based-Context"}, captured
    assert json.loads(captured[0]["X-Based-Context"]) == {
        "owner": settings["callers"]["viewer"]["owner"], "role": "viewer"}
