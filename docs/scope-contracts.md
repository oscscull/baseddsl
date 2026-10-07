# Scope isolation contract matrix

Scope tests assert visible rows and persisted effects with deliberately divergent
owners and relation targets. Stacked decorators declare alternative ownership
paths; a callable chooses named axes, whose predicates are ANDed. They do not
create a runtime OR over every declared axis. Missing required context fails
before execution. Explicit `unscoped("reason")` sites forfeit scope enforcement.

| Behavior | Executable evidence |
|---|---|
| Root tenant reads | `sqlite_integration::commerce::ctx_scoped_list_query_binds_context`; live MariaDB `queries::ctx_scoped_list_filters_by_org` |
| Joined to-one, cross-scope target | `sqlite_integration::bulk_deletes::joined_scope_hides_cross_scope_row_end_to_end` |
| Nested to-one and to-many, divergent region | `sqlite_integration::scopes::nest_reached_scoped_child_is_confined_cross_tenant` returns null/empty for foreign targets |
| OR alternatives vs AND axes; missing context | `sqlite_integration::scopes::scope_alternatives_select_one_axis_and_named_axes_intersect` deliberately seeds all four combinations |
| Update confinement on both axes | `sqlite_integration::scopes::scoped_update_cannot_change_either_foreign_axis` checks 404 and unchanged foreign values directly in the DB |
| Bulk delete confinement | `sqlite_integration::bulk_deletes::scoped_delete_all_wipes_only_the_callers_scope_end_to_end` |
| Bulk create ownership | `bulk_integration::bulk_insert_injects_scope_from_ctx_not_the_payload` |
| Scoped nested/bulk create, both child directions | `bulk_integration::bulk_to_many_nested_write_fans_children_to_the_right_parent` seeds another tenant's parent/customer/items, checks caller fan-out and foreign data remains unchanged |
| Optional context and nullable owner | `optional_ctx_integration::anonymous_caller_never_sees_a_private_owned_deck` includes the **unowned private** row; signed-in companion checks both owners |
| Raw predicate leaf | `sqlite_integration::scopes::raw_leaves_keep_root_scope_but_raw_subqueries_and_bodies_own_their_filters` proves an always-true OR cannot bypass injected root scope |
| Raw expression subquery | Same test: outer rows are scoped, but the raw count sees both tenants. Raw SQL authors must scope its internal tables |
| Whole raw body / explicit unscoped | Same test intentionally returns both tenants; sema `raw_queries::raw_query_on_scoped_model_must_be_unscoped` rejects a false scoped promise |
| Optional context in scope/write | Sema `optional_context::optional_ctx_in_scope_term_is_rejected` and `optional_ctx_in_mutation_filter_is_rejected` reject with E0339 rather than silently owning/matching NULL |
| Explicit scope-column assignment | Sema `scopes::create_assigning_scope_column_is_e0181` reject with E0181; ownership is injected from trusted context |

Test sources: [nested reads and scope composition](../crates/based-runtime/tests/sqlite/sqlite_integration/scopes.rs),
[joined reads and deletes](../crates/based-runtime/tests/sqlite/sqlite_integration/bulk_deletes.rs),
[bulk/nested writes](../crates/based-runtime/tests/sqlite/bulk_integration.rs),
[optional context](../crates/based-runtime/tests/sqlite/optional_ctx_integration.rs),
[raw-body compiler checks](../crates/based-sema/tests/check/raw_queries.rs),
[optional-context compiler checks](../crates/based-sema/tests/check/optional_context.rs),
and [scope-column compiler checks](../crates/based-sema/tests/check/scopes.rs).
Runtime tests are modules of `crates/based-runtime/tests/sqlite_all.rs`; compiler
checks are modules of `crates/based-sema/tests/check.rs`. Existing
[SQL-lowering tests](../crates/based-codegen/tests/dml/scopes.rs) retain per-dialect
predicate coverage. The new adversarial cases use real SQLite rather than a new
full cross-product of dialects and features. Live server suites retain their
representative joined-scope/read/write tests.

Run `cargo test -p based-runtime --features sqlite --test sqlite_all scopes::`
and `cargo test -p based-runtime --features sqlite --test sqlite_all bulk_to_many_nested_write`
for the new gaps. `make check-fast` includes all SQLite/compiler cases;
`make check` additionally lints all features and runs the live suites/examples.
Fresh typed-client contracts are tracked separately in issue #52.

Nullable ownership is a policy decision: absent `$ctx.user?` equals SQL NULL,
which matches unowned rows even when they are private. A public-only anonymous
feed must explicitly enforce visibility. Scope is row confinement on modeled
operations, not a sandbox for raw SQL or a database foreign-key authorization
policy. Audit explicit unscoped sites and raw subqueries before exposing them.
