"""Own the local edge and callback listener lifetimes."""
from contextlib import contextmanager
from http.server import ThreadingHTTPServer
from threading import Thread

from callback import Callback
from edge import Edge
from settings import address


@contextmanager
def serving(settings):
    servers = {}
    try:
        for name, handler in (("callback", Callback), ("edge", Edge)):
            server = ThreadingHTTPServer(address(settings, name), handler)
            server.settings = settings
            servers[name] = server
            Thread(target=server.serve_forever, daemon=True).start()
        yield servers
    finally:
        for server in reversed(list(servers.values())):
            server.shutdown()
            server.server_close()
