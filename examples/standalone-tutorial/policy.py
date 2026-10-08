"""Decide the named v1 permission check without reading or changing database state."""
from settings import OWNER, OTHER_OWNER


def verdict(request):
    if not isinstance(request, dict) or set(request) != {"version", "guard", "callable", "args", "ctx"}:
        return deny()
    args, ctx = request["args"], request["ctx"]
    if type(request["version"]) is not int or request["version"] != 1:
        return deny()
    if request["guard"] != "caller_can_rename" or request["callable"] != "rename_item":
        return deny()
    if not isinstance(args, dict) or not isinstance(ctx, dict):
        return deny()
    if ctx.get("owner") not in (OWNER, OTHER_OWNER) or ctx.get("role") != "editor":
        return deny()
    if set(args) != {"id", "name", "expected_name"} or not all(isinstance(value, str) for value in args.values()):
        return deny()
    return {"version": 1, "verdict": "allow"}


def deny():
    return {"version": 1, "verdict": "deny", "message": "Editor permission required"}
