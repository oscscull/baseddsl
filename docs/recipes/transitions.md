# Keep a write set atomic and enforce a state transition

Task: create related tutorial rows together. Add this callable to the tutorial schema:

```bsl
mutation create_pair(parent_name: text, child_name: text) -> ItemView scoped Author {
  tx {
    create Item { name = $parent_name } as parent;
    create Item { name = $child_name, parent = $parent.id };
  }
}
```

The static write set commits together or rolls back together. `$parent.id` reads
the created row within that transaction. The DSL does not branch on a read value;
read-decide-write logic belongs in a host transaction.

For a guarded transition, use the standalone lesson's minimal conditional write:

```bsl
mutation rename_item(id: Id, name: text, expected_name: text) -> ItemView guard caller_can_rename scoped Author {
  update Item where (id = $id and name = $expected_name) { name = $name };
}
```

The named guard decides permission **before** the write transaction. Its state
reads use another connection, including for adopted transactions; they cannot see
uncommitted caller state or hold its row locks. The conditional predicate makes
the expected state part of the write. A stale/absent/out-of-scope row returns
`404 not_found` with no transition. A preflight denial returns `403 guard_denied`.
Guard registration is required before an engine/service can start.

Follow the [standalone lesson](../standalone-tutorial.md) for the concrete callback;
the [auth contract](../../spec/syntax/auth.md#preflight-permissions-and-atomic-invariants)
is authoritative for preflight ordering and invariant boundaries, while
[external guard v1](../../spec/external-guards.md) specifies its wire adapter.
The helpdesk's [close policy](../../examples/axum-helpdesk/src/close_policy.rs) and
[conditional close](../../examples/axum-helpdesk/schema/ticket/queries.bsl) demonstrate
the same separation with an adversarial concurrent writer.

Use [host transactions](../../spec/syntax/transactions.md) for locking
read-decide-write. `for update` requires a transaction-bound client and gives row
locks on supported server databases; it is a documented no-op on SQLite, whose
transaction locking differs. An HTTP transaction cannot span requests.
