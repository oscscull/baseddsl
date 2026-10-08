"""Exercise the lesson's HTTP contract without ambient proxies or redirects."""
from http.client import HTTPConnection
import json


def request(settings, path, payload, identity="editor", key=None, spoof=False, listener="edge"):
    headers = {"Content-Type": "application/json"}
    if identity:
        headers["Authorization"] = f'Bearer {settings["callers"][identity]["token"]}'
    if key:
        headers["Idempotency-Key"] = key
    if spoof:
        headers["X-Based-Context"] = json.dumps({"owner": settings["callers"]["other"]["owner"], "role": "editor"})
        headers["X-Based-Shard-Key"] = "attacker-controlled"
    connection = HTTPConnection("127.0.0.1", settings["ports"][listener], timeout=6)
    try:
        connection.request("POST", path, body=json.dumps(payload).encode(), headers=headers)
        response = connection.getresponse()
        return response.status, json.loads(response.read())
    finally:
        connection.close()


def success(*args, **kwargs):
    status, result = request(*args, **kwargs)
    assert status == 200, (status, result)
    return result
