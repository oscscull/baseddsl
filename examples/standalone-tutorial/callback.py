"""Serve authenticated named permission checks on a private loopback listener."""
import json

from auth import callback_authorized
from http_io import JsonHandler
from policy import verdict


class Callback(JsonHandler):
    def do_POST(self):
        if self.path != "/check":
            return self.reply(404, {"error": "not found"})
        if not callback_authorized(self.headers.get("Authorization"), self.server.settings["guard_secret"]):
            return self.reply(401, {"error": "callback authentication required"})
        try:
            request = json.loads(self.body())
        except (ValueError, TimeoutError):
            return self.reply(400, {"error": "invalid JSON request"})
        self.reply(200, verdict(request))
