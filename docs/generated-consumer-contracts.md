# Fresh generated-client contracts

`ci/generated-consumers.py` creates a temporary standalone Cargo project outside
the workspace, invokes the built `based gen client --embedded`, compiles with
explicit consumer dependencies, then executes typed calls through the real engine.
Every run starts with fresh client bytes. It also checks the four committed
example clients through `based gen client --check` and rejects interchanged entity IDs via a negative
compile fixture. Compiler failures must specifically identify the Owner/Item type
mismatch; unrelated compiler failures cannot pass that test.

| Contract | Compact fixture assertion |
|---|---|
| Singular mutation | Created owner/item fields decode exactly |
| Optional get | Existing ID returns a row; absent/deleted ID returns None |
| List, dynamic arrays | Empty, singleton and two-value filters return exact names |
| Page | Two offset pages contain A then B, with total = 2 |
| Stream | Typed rows arrive in order, then EOF |
| Unit | Delete succeeds; a typed get proves the row is gone |
| Nullable and nested JSON | Structured JSON survives create/get/to-many nest; SQL NULL becomes None; scalar JSON strings stay strings |
| Typed IDs | Returned item ID feeds an item query; owner ID in its place fails compilation |
| Exact decimals | Positive/negative 38-digit integer/fraction boundary values round-trip |

The same small schema runs on SQLite, MariaDB and Postgres. This deliberately
avoids a feature-by-dialect Cartesian product; existing focused compiler/runtime
tests cover other lowering variants. The consumer backend setup drops `item` and
`owner`, then executes generated DDL. Only use disposable test databases.

For contributors: iterate with `make check-fast`. Execution changes still require
`make check` (all-feature lint, workspace tests, live databases and examples).
Client generation, serialization or embedded transport changes additionally need:

```sh
make ci-generated-consumer-sqlite
make ci-generated-consumer-mariadb ci-generated-consumer-postgres
```

The server targets use the disposable local servers from `make dev-db-up`; override
`MARIADB_URL`/`POSTGRES_URL` for provided servers. Python 3 and Cargo are required.
The initial consumer build resolves and fetches its own dependencies, so it works
with a cold Cargo cache. The negative ID check then uses that lockfile offline.
Consumers share only a compilation cache under `target/consumer-contract`,
not source files, generated artifacts or database state. This expensive gate runs
in its own CI job, preserving the fast workspace build budget.

The fresh MariaDB fixture exposed binary-collated JSON arriving as base64 because
the MySQL driver reports its LONGTEXT storage as a blob. Runtime shaping now
unwraps that representation only at schema-declared JSON/aggregate-array positions;
actual bytes columns retain their base64 wire contract.
