# Select one ownership alternative or require both axes

Task: declare which ownership axes each modeled callable must enforce. This
independent minimal schema uses UUID values supplied by trusted host authentication:

```bsl
scope Tenant (org: uuid = $ctx.org)
scope Author (owner: uuid = $ctx.user)

@scope Tenant
@scope Author
Post {
  id: Id
  org: uuid
  owner: uuid
  body: text
  @index org
  @index owner
}
query org_posts() -> Post[] scoped Tenant { list Post order (id); }
query my_posts() -> Post[] scoped Author { list Post order (id); }

@scope Tenant, Author
Draft {
  id: Id
  org: uuid
  owner: uuid
  body: text
  @index(org, owner)
}
query my_org_drafts() -> Draft[] scoped Tenant, Author { list Draft order (id); }
```

Two decorators are **OR alternatives at declaration time**: `org_posts` chooses
the org axis; `my_posts` chooses the owner axis. Neither query dynamically returns
the union of both sets. One decorator with comma-separated names is **AND**:
drafts require both predicates, and a callable naming only Tenant is rejected.
Naming both axes on the OR model also intersects their row sets.

Required context must be present; otherwise execution fails with `missing_ctx`.
Creates auto-set scope columns from context and cannot explicitly assign them.
A create must satisfy a whole declared alternative and supply required non-null
ownership fields. `unscoped("reason")` explicitly forfeits scope injection; host
authentication must authorize that crossing. Incorrect host context can select
another owner's rows. Raw subqueries and whole raw bodies own their internal
filters; see [raw boundaries](raw.md).

The advanced helpdesk uses this pattern in
[Ticket](../../examples/axum-helpdesk/schema/ticket/model.bsl) and
[DraftNote](../../examples/axum-helpdesk/schema/draft/model.bsl).
[Scope contracts](../scope-contracts.md) link adversarial runtime evidence;
[auth semantics](../../spec/syntax/auth.md) define the full compiler rule.
