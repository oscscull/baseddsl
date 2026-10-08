# Standalone tutorial: an authenticated edge and durable replay

Run the same small `Item` model, optional parent relation, and owner scope as the
[embedded tutorial](embedded-tutorial.md), with a native Based service behind a
concrete trusted edge. The Python standard-library helpers need no Rust or npm
source build. This lesson adds one named permission check and one conditional
write; it requires no identity provider.

## Install matching artifacts

Follow [installation](installation.md) to obtain a verified native `based` binary
and **`based-standalone-tutorial-0.1.12.zip` from the same artifact set**. Verify
`SHA256SUMS` before extracting. Use Python 3.12 or newer (`python3` on Unix,
`py -3` on Windows). Put `based` on `PATH`; its version and source commit must
match `tutorial/manifest.json`. An older 0.1.12 candidate lacks this tutorial.
Until the owner publishes a release, use the verified release-dry-run artifacts
from the same CI run. No public release is claimed here.

Create an empty directory:

```sh
mkdir standalone-items
cd standalone-items
based init --mode standalone
```

Extract the verified lesson zip **here**. It creates `tutorial/` without replacing
initialized files. On Unix:

```sh
unzip /absolute/path/based-standalone-tutorial-0.1.12.zip
python3 tutorial/setup.py
based check
based gen all
based migrate apply --database-url local.db
python3 tutorial/run.py
```

On Windows use `Expand-Archive` into the current directory and substitute `py -3`.
Setup refuses existing lesson configuration. Random local caller tokens and a
separate callback secret are stored in ignored `tutorial/.env.json`, with
owner-only permissions on Unix. Protect it with your platform's file permissions.
Owner UUIDs are scope values, not passwords. Setup does not apply migrations.

Keep the server terminal open. Use a second terminal in the same project for the
requests below. On reruns, retain the database, migration history, and lesson
configuration. Ctrl-C stops the edge, callback, and Based process.

## The trusted boundary

All listeners bind literal loopback:

| Listener | Default | Access |
| --- | --- | --- |
| Authenticated edge | `127.0.0.1:8080` | Operator-issued caller bearer tokens |
| Based backend | `127.0.0.1:9090` | Private to the trusted host |
| Named callback | `127.0.0.1:9100/check` | Separate bearer secret supplied only by Based |

Loopback prevents remote clients from directly reaching the backend or callback.
This local lesson **trusts other processes on the host**. Deploy on a trusted host
with firewall rules, or isolate the backend and callback in a private network
accessible only to the edge. Expose only the edge through TLS; never publish the
backend/callback ports. Follow [deployment](standalone-deployment.md) for TLS,
limits, credentials, readiness, shutdown, and network isolation.

`auth.py` maps provisioned tokens to an owner and role. Replace that boundary
with your application's established authentication when deploying. The edge
constructs fresh downstream headers: JSON content type, authenticated
`X-Based-Context`, and an optional idempotency key. It copies neither caller
authorization nor caller-supplied `X-Based-Context`, `X-Based-Shard-Key`, or other
`X-Based-*` headers. Only the four lesson routes are exposed.

The initializer supplies [the canonical schema](../crates/based-cli/starter/schema/item.bsl).
Setup adds [this supplemental mutation](../examples/standalone-tutorial/guarded-rename.bsl):

```bsl
mutation rename_item(id: Id, name: text, expected_name: text) -> ItemView guard caller_can_rename scoped Author {
  update Item where (id = $id and name = $expected_name) { name = $name };
}
```

Owner scope filters database reads and writes. `caller_can_rename` checks the
host-derived `editor` role. The `expected_name` predicate enforces the state
invariant atomically in the write. The permission callback performs no database
reads or writes and cannot replace that conditional write.

`guards.toml` configures the named endpoint, `BASED_TUTORIAL_GUARD_SECRET`, and a
two-second deadline. `backend.py` supplies the secret in the **Based process
environment**; project `.env` files do not supply callback secrets. The callback
authenticates before reading JSON and implements the final
[v1 contract](../spec/external-guards.md): request fields `version`, `guard`,
`callable`, `args`, `ctx`; HTTP 200 with `{"version":1,"verdict":"allow"}` or
`deny` with a bounded public message. Invalid permission requests fail closed.
[Guard configuration](standalone-guards.md) explains verified HTTPS, transport
bounds, and the explicit development-only loopback HTTP exception.

## Create and read through the edge

The client reads tokens privately from configuration; it does not put them in
arguments or print them. Call the documented HTTP interface:

```sh
python3 tutorial/client.py editor /m/create_item '{"name":"Parent","parent":null}' --key lesson-parent-1
```

Copy the returned ID. Replace `PARENT_ID` below:

```sh
python3 tutorial/client.py editor /m/create_item '{"name":"Child","parent":"PARENT_ID"}' --key lesson-child-1
python3 tutorial/client.py editor /q/items '{}'
```

Pass each JSON payload as one intact shell argument. HTTP 200 responses
contain JSON `ItemView` objects or arrays, including `parent: {id, name}` for the
child. `generated/openapi.json` describes the exact request/response types.
`/q/item_by_id` accepts `{"id":"CHILD_ID"}`. Calling `items` as `other` returns
only that other owner's rows.

## Guarded conditional write and durable replay

Replace `CHILD_ID` and retain the same payload and key:

```sh
python3 tutorial/client.py editor /m/rename_item '{"id":"CHILD_ID","name":"Renamed","expected_name":"Child"}' --key lesson-rename-1
python3 tutorial/client.py editor /m/rename_item '{"id":"CHILD_ID","name":"Renamed","expected_name":"Child"}' --key lesson-rename-1
```

Both return the same saved `ItemView`; the second call does not execute the
conditional write again. Stop `run.py`, start it again, and repeat the exact
request. The saved result survives in the same `local.db`. Reusing a key with a
different payload is a conflict. Choose a new key for each intended logical write.

Use `viewer` for that request, even with the saved key: HTTP 403,
`error.code = "guard_denied"`. Guards run before replay lookup, so saved success
cannot bypass current permission. Use `editor`, a **new** key, and stale
`expected_name: "Child"`: HTTP 404 `not_found`, with the name still `Renamed`.
No-match also covers absent or out-of-scope rows, without distinguishing them.
If the callback is unavailable or its secret is wrong, the guarded write fails
closed with 403 before any write or key claim.

The runner uses these explicit service settings and supplies the secret environment:

```sh
based serve --listen 127.0.0.1:9090 --database-url local.db \
  --idempotency-store database --init-idempotency-table \
  --guard-config guards.toml --guard-allow-loopback-http
```

Database replay commits the write and replay record atomically, retaining keys
until explicitly deleted. Here `--init-idempotency-table` deliberately provisions
the local table. For production, provision it per shard through your migration
process and remove that flag. The initializer's short `demo.py` uses memory
replay; this lesson uses **database** replay. See
[idempotency](standalone-idempotency.md) for retention and deployment behavior.

## Evolve the same schema without losing data

Stop `run.py` before migration. Follow the embedded tutorial's
[rename](embedded-tutorial.md#rename-a-field-without-losing-data): in `item.bsl`,
rename the model field to `title: text @was("name")`, project `name = title` in
`ItemView` and its nested parent, and assign `title = $name` in `create_item`.
Keep the callable input and JSON field `name` stable.

Also edit `schema/guarded-rename.bsl`: use `title = $expected_name` in the predicate
and `{ title = $name }` for the assignment. Then:

```sh
based check
based gen all
based migrate gen . rename_item_title
based migrate verify
based migrate render . --number 2
```

Review the rename SQL before applying it:

```sh
based migrate apply --database-url local.db
based gen all --check
python3 tutorial/run.py
```

Generation alone does not change the database. Migration generation consumes the
`@was` hint; retain the resulting immutable history. Rows, owner values, and parent
links survive. The same payloads and projected JSON work, including saved keyed
results. Adding a model field does not expose it unless you select it in a shape.

For a reset choose a **new project directory**, retaining the old database and
history. Never delete a live SQLite file or sidecars as an implicit reset. SQLite
has its usual single-writer constraints. MariaDB/PostgreSQL use the same schema
but need provisioned servers and corresponding backend configuration; this lesson's
runner is deliberately SQLite-specific. See [support](support-policy.md).

Assertions are separate: `make ci-onboarding` covers both tutorials;
`make ci-standalone-tutorial` checks authentication,
spoofed headers, scoped relations, guard failures, restart replay, stale conditions,
and data-preserving regeneration. The release collector repeats it with extracted
native/lesson artifacts and no Rust/npm build. Continue with the
[reference](reference.md) and deployment guide.
