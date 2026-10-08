"""Expose the documented tutorial calls after authentication and context replacement."""
from http.client import HTTPException

from auth import authenticate
from forward import call
from http_io import JsonHandler
from settings import address

ROUTES = {"/m/create_item", "/m/rename_item", "/q/items", "/q/item_by_id"}


class Edge(JsonHandler):
    def do_POST(self):
        context = authenticate(self.headers.get("Authorization"), self.server.settings["callers"])
        if context is None:
            return self.reply(401, {"error": "caller authentication required"})
        if self.path not in ROUTES:
            return self.reply(404, {"error": "not found"})
        if self.path == "/m/create_item" and context["role"] != "editor":
            return self.reply(403, {"error": "Editor permission required"})
        try:
            data = self.body()
        except (ValueError, TimeoutError):
            return self.reply(400, {"error": "invalid request body"})
        try:
            status, response = call(address(self.server.settings, "backend"), self.path,
                                    data, context, self.headers.get("Idempotency-Key"))
        except (OSError, ValueError, HTTPException):
            return self.reply(502, {"error": "private backend unavailable"})
        self.send_bytes(status, response)
