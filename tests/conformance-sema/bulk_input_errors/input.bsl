# BW1 shape-as-input eligibility, checked at the use site (where a shape meets `create
# Model`). Each mutation below trips a distinct diagnostic.
Org { id: Id, name: text }
scope Tenant (org: Org = $ctx.org)

Category { id: Id, name: text }

@created(created_at)
@updated(updated_at)
@scope Tenant
Product {
  id: Id
  org: Org
  category: Category
  sku: text
  name: text
  price: int
  created_at: timestamp
  updated_at: timestamp
  @index(org)
  @index(category)
}

# Missing the required `name` and `price` columns -> E0330.
shape MissingCols from Product { sku }
mutation m_missing(rows: MissingCols[]) -> ok scoped Tenant { create Product[] from $rows; }

# A computed field has no column to write -> E0327.
shape Computed from Product { sku, name, price, tag = sku || name }
mutation m_computed(rows: Computed[]) -> ok scoped Tenant { create Product[] from $rows; }

# A bare relation (no `{ key }` block) is not settable -> E0328.
shape BareRel from Product { sku, name, price, category }
mutation m_bare(rows: BareRel[]) -> ok scoped Tenant { create Product[] from $rows; }

# A relation block naming non-key payload is a nested write -> E0329 (reserved).
shape NestedWrite from Product { sku, name, price, category { id, name } }
mutation m_nested(rows: NestedWrite[]) -> ok scoped Tenant { create Product[] from $rows; }

# Naming the engine-managed `@created`/`@updated` timestamps — legal, warned (W0112): the
# payload value is written verbatim, overriding the automatic now() stamp.
shape NamesManaged from Product { sku, name, price, category { id }, created_at, updated_at }
mutation m_managed(rows: NamesManaged[]) -> ok scoped Tenant { create Product[] from $rows; }

# A bulk read-back (BW1b): `create Model[] from $rows -> Shape[]` is valid — the written
# rows read back in the declared shape (arity matches the `[]`). No diagnostic.
shape Ok from Product { sku, name, price, category { id } }
mutation m_readback(rows: Ok[]) -> ProductRow[] scoped Tenant { create Product[] from $rows; }
shape ProductRow from Product { sku, name }

# Return-arity mismatch: a bulk `Model[] from` reads back `-> Shape[]` (or `-> ok`), not a
# scalar `-> Shape` -> E0332.
mutation m_arity_ret(rows: Ok[]) -> ProductRow scoped Tenant { create Product[] from $rows; }

# Wrong param arity: a bulk `Model[] from` needs a `shape[]` param -> E0325.
mutation m_arity(row: Ok) -> ok scoped Tenant { create Product[] from $row; }
