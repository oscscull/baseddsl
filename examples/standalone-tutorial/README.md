# Standalone tutorial helpers

Follow the [standalone tutorial](https://github.com/oscscull/baseddsl/blob/main/docs/standalone-tutorial.md).
Use the matching native CLI and `based-standalone-tutorial-VERSION.zip` from the same
verified artifact set. The archive includes these Python standard-library programs,
this supplemental mutation, the license, and exact source metadata. It contains no
credentials and requires no Rust or npm build.

`based init --mode standalone` supplies the shared `Item` schema. These helpers add
a local authenticated edge and named permission callback. `setup.py` provisions
random operator tokens in ignored `tutorial/.env.json`; it refuses existing lesson
files. Owner UUIDs are scope values, not credentials. Keep this file private.

Every listener binds loopback. This demonstration trusts other processes on the
host. Deploy behind TLS on a trusted host or isolate the backend and callback in a
private network accessible only to the edge. Never publish their ports.

Responsibilities: `auth.py` authenticates; `policy.py` decides permission;
`callback.py` serves that policy; `edge.py` exposes selected authenticated calls;
`forward.py` replaces downstream headers; `http_io.py` bounds HTTP messages;
`backend.py` manages Based; `listeners.py` owns edge/callback lifetimes;
`run.py` coordinates them; `client.py` demonstrates ordinary HTTP calls;
`settings.py` loads operator configuration; `setup.py` creates it.

These are local learning helpers, not an identity provider or a production web
framework. Executable assertions live separately in `ci/standalone_tutorial/`.
