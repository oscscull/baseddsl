"""Forward an authenticated call using only explicitly constructed downstream headers."""
from http.client import HTTPConnection
import json


def call(address, path, data, context, key):
    headers = {"Content-Type": "application/json", "X-Based-Context": json.dumps(context)}
    if key:
        headers["Idempotency-Key"] = key
    connection = HTTPConnection(*address, timeout=4)
    try:
        connection.request("POST", path, body=data, headers=headers)
        response = connection.getresponse()
        body = response.read(1024 * 1024 + 1)
        if len(body) > 1024 * 1024:
            raise ValueError("backend response too large")
        return response.status, body
    finally:
        connection.close()
