# Recipes by task

Start with the [embedded](embedded-tutorial.md) or [standalone](standalone-tutorial.md)
tutorial. These focused recipes extend that small example or point to a working
advanced example. Each states the boundary alongside its minimal code.

| Task | Recipe |
| --- | --- |
| Return a parent alongside a row | [Relations and shapes](recipes/relations.md) |
| Confine rows by one ownership alternative or two required axes | [Scope OR/AND composition](recipes/scopes.md) |
| Read a feed without signed-in context | [Anonymous context and nullable ownership](recipes/anonymous.md) |
| Write related rows atomically or enforce a state transition | [Transactions and conditional writes](recipes/transitions.md) |
| Retry a request or a serialization-aborted transaction | [Replay and transaction retries](recipes/retries.md) |
| Add one SQL expression or take responsibility for a whole query | [Raw boundaries](recipes/raw.md) |
| Use an existing pool or caller-owned transaction | [Pool and transaction integration](recipes/pools.md) |
| Inspect generated parameterized SQL | [SQL inspection](recipes/sql.md) |

Authentication supplies trusted context; scope confines modeled operations using
that context. Raw bodies, raw subqueries, explicit unscoped sites, and host SQL
must be reviewed separately. Scope is not an authorization sandbox.

The [helpdesk](../examples/axum-helpdesk/README.md) is an advanced tour combining
these ideas, with server provisioning and demo authentication; it is not a first-run
prerequisite. Keep [migration](../spec/syntax/migrations.md),
[generation](generated-artifacts.md), and [deployment](standalone-deployment.md)
operations in their guides rather than reproducing them in every recipe.
