"""Call the edge using a selected operator-issued demonstration identity."""
import argparse
from http.client import HTTPConnection
import json

from settings import address, load


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("identity", choices=("editor", "viewer", "other"))
    parser.add_argument("path", choices=("/m/create_item", "/m/rename_item", "/q/items", "/q/item_by_id"))
    parser.add_argument("payload", help="JSON object matching the generated OpenAPI request")
    parser.add_argument("--key")
    args = parser.parse_args()
    settings = load()
    headers = {"Authorization": f'Bearer {settings["callers"][args.identity]["token"]}',
               "Content-Type": "application/json"}
    if args.key:
        headers["Idempotency-Key"] = args.key
    connection = HTTPConnection(*address(settings, "edge"), timeout=5)
    try:
        connection.request("POST", args.path, body=json.dumps(json.loads(args.payload)), headers=headers)
        response = connection.getresponse()
        print(f"HTTP {response.status}")
        print(response.read().decode())
        if response.status >= 400:
            raise SystemExit(1)
    finally:
        connection.close()


if __name__ == "__main__":
    main()
