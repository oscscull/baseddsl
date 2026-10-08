"""Authenticate operator-provisioned local example tokens into host context."""
import hmac


def authenticate(header, callers):
    if not header or not header.startswith("Bearer "):
        return None
    token = header.removeprefix("Bearer ")
    for caller in callers.values():
        if hmac.compare_digest(token.encode(), caller["token"].encode()):
            return {"owner": caller["owner"], "role": caller["role"]}
    return None


def callback_authorized(header, secret):
    return hmac.compare_digest((header or "").encode(), f"Bearer {secret}".encode())
