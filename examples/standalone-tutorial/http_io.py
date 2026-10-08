"""Bound JSON HTTP input/output for the tutorial's two small listeners."""
from http.server import BaseHTTPRequestHandler
import json

MAX_BODY = 256 * 1024


class JsonHandler(BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(3)

    def log_message(self, *_args):
        pass  # Never record credentials, context, or request bodies.

    def body(self):
        if self.headers.get("Transfer-Encoding") or len(self.headers.get_all("Content-Length", [])) != 1:
            raise ValueError("one Content-Length required")
        size = int(self.headers["Content-Length"])
        if not 0 <= size <= MAX_BODY:
            raise ValueError("body too large")
        data = self.rfile.read(size)
        if len(data) != size:
            raise ValueError("incomplete body")
        return data

    def reply(self, status, payload):
        self.send_bytes(status, json.dumps(payload).encode())

    def send_bytes(self, status, data):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)
        self.close_connection = True
