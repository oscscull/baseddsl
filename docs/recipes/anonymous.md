# Read a feed when identity is absent

Task: let a query accept absent host identity without accidentally treating it as
an unrestricted match. This independent example deliberately has nullable ownership:

```bsl
Post {
  id: Id
  author: uuid?
  visibility: text
  @index author
  @index visibility
}
query feed() -> Post[] {
  list Post where (author = $ctx.user? or visibility = "public") order (id);
}
query public_posts() -> Post[] {
  list Post where (visibility = "public") order (id);
}
```

Absent optional context binds SQL NULL. Null-safe equality makes the first leaf
match `author IS NULL`, not every row. Consequently anonymous `feed` returns
**public rows and unowned private rows**. With required non-null ownership it
returns only public rows. Signed-in callers get their own rows plus public ones.
For public-only anonymous access regardless of ownership nullability, call
`public_posts`; do not assume optional identity enforces that policy.

The generated optional context field is `Option<T>`; `None` binds NULL. This
construct is query-filter-only, rejected in scopes, mutations, and named filters
that can enter writes. Missing required `$ctx.user` instead yields `missing_ctx`.
Host authentication determines when identity is absent or present.

See [optional context semantics](../../spec/syntax/auth.md#optional-context--ctxfield)
and the [nullable-owner runtime cases](../../crates/based-runtime/tests/sqlite/optional_ctx_integration.rs).
