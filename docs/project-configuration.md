# Project discovery and local connections

Every CLI command selects the same project. Without a root argument, Based
searches the current directory and its ancestors for the nearest `based.toml`.
With a root argument, only that directory's manifest is used: an explicit root
with no manifest fails instead of inheriting a parent project. Use an explicit
root to work on another project from any directory.

Schema `root`, migration storage, relative generated `-o` destinations and
SQLite database file paths resolve relative to the selected manifest directory.
Absolute paths remain absolute. SQLite `:memory:` retains its special meaning.
For example, from `src/`, `based gen client -o generated/client.rs --embedded`
writes the project's `generated/client.rs`; `based migrate gen` writes the
project's `migrations/`. Relative `-o` paths previously depended on invocation
cwd: scripts generating into a different directory should supply an absolute
output path. The CLI does not create missing output directories yet.

Only live commands (`migrate apply`, `migrate status`, `serve`) use connection
configuration. The order is:

1. Explicit repeated `--database-url` flags.
2. Existing process `BASED_DATABASE_URL`, then process `DATABASE_URL`.
3. Selected project `.env`: `BASED_DATABASE_URL`, then `DATABASE_URL`.

A URL variable accepts comma-separated values. Empty values fail with a
configuration diagnostic rather than selecting another layer. In particular,
process `DATABASE_URL` wins over `.env` `BASED_DATABASE_URL`. SQLite serve permits
one file path rather than a server URL. `--pool-max` must be positive and
`--pool-min` must not exceed it. Based parses only the selected project's `.env`, does not search parent
or invoking directories for it, and does not mutate process variables. A missing
file is fine; malformed/unreadable local config fails when that layer is needed,
with values redacted. Higher-priority connections do not depend on `.env` being
valid. Dotenv interpolation follows dotenvy's rules, including existing process
variables; keep connection assignments explicit for predictable deployment.

Offline `check`, `fmt`, `gen`, `facts` and migration `gen`/`render`/`verify` need
no connection and do not read `.env`. Keep local secrets and database files out
of source control. Configuration errors name the file or option to correct,
without printing connection credentials.

Manifest defaults apply only to omitted choices. Explicit dialects must be
`mariadb`, `mysql`, `sqlite`, `postgres` or `postgresql`; the client target must
be `rust`; `[schema] id` must be `uuid`, `ulid` or `serial`; and
`[schema] foreign_keys` must be `all` or `none`. Unknown manifest fields are
rejected to catch misspelled options. `migrate render --dialect` also validates
its override. A missing/empty/unreadable schema is a diagnostic, never a partial
project silently compiled after skipped filesystem errors.
