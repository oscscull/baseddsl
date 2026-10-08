"""Verify authenticated context replacement and permission failures through the edge."""
from requests import request, success


def verify(settings):
    assert request(settings, "/q/items", {}, identity=None)[0] == 401
    assert request(settings, "/check", {}, identity=None, listener="callback")[0] == 401
    parent = success(settings, "/m/create_item", {"name": "Parent", "parent": None})
    item = success(settings, "/m/create_item", {"name": "Child", "parent": parent["id"]})
    assert item["parent"] == {"id": parent["id"], "name": "Parent"}
    other = success(settings, "/m/create_item", {"name": "Private", "parent": None}, identity="other")
    rows = success(settings, "/q/items", {}, spoof=True)
    assert {row["id"] for row in rows} == {parent["id"], item["id"]}
    assert other["id"] not in {row["id"] for row in rows}
    payload = {"id": item["id"], "name": "Denied", "expected_name": "Child"}
    status, error = request(settings, "/m/rename_item", payload, identity="viewer", spoof=True)
    assert status == 403 and error.get("error", {}).get("code") == "guard_denied", (status, error)
    assert success(settings, "/q/item_by_id", {"id": item["id"]})["name"] == "Child"
    return item
