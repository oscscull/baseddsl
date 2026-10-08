"""Verify a durable keyed conditional write across a real Based process restart."""
from requests import request, success


def rename(settings, item):
    payload = {"id": item["id"], "name": "Renamed", "expected_name": "Child"}
    first = success(settings, "/m/rename_item", payload, key="lesson-rename-1")
    assert first["name"] == "Renamed", first
    assert success(settings, "/m/rename_item", payload, key="lesson-rename-1") == first
    status, error = request(settings, "/m/rename_item", payload, identity="viewer", key="lesson-rename-1", spoof=True)
    assert status == 403 and error.get("error", {}).get("code") == "guard_denied", (status, error)
    return payload, first


def restarted(settings, payload, first):
    assert success(settings, "/m/rename_item", payload, key="lesson-rename-1") == first
    status, result = request(settings, "/m/rename_item", payload, key="lesson-stale-write")
    assert status == 404 and result["error"]["code"] == "not_found", (status, result)
    assert success(settings, "/q/item_by_id", {"id": payload["id"]})["name"] == "Renamed"
    print("stale conditional write:", status, result)


def unavailable(settings, payload):
    status, error = request(settings, "/m/rename_item", payload, key="lesson-rename-1")
    assert status == 403 and error.get("error", {}).get("code") == "guard_denied", (status, error)
