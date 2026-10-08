"""Coordinate the local lesson's trusted edge, callback, and private Based process."""
import os

import backend
from listeners import serving
from settings import load


def main():
    settings = load()
    with serving(settings):
        process = backend.start(settings, os.environ.get("BASED", "based"))
        try:
            print(f'Authenticated edge: http://127.0.0.1:{settings["ports"]["edge"]}', flush=True)
            code = process.wait()
            raise RuntimeError(f"Based exited with status {code}")
        except KeyboardInterrupt:
            pass
        finally:
            backend.stop(process)


if __name__ == "__main__":
    main()
