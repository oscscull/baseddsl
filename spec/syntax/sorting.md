# syntax/sorting.md

Principles: 1 (default + explicit override), 2 (unordered = consequential), 4.

Sort is a property of rows (model/relation), NOT of projection. Shapes carry no sort.

## Precedence (most-specific wins)
query `order (...)`  >  relation `@sort`  >  model `@sort`  >  none -> lint

## Model default
Absent any other instruction, the entity lists in this order. Closes the bare-form gap (bare queries have nowhere to write `order`, so the default must live on the data model).
```
@sort(created_at desc)
Post { ... }
```

## Relation default (overrides model, for that traversal)
"This entity when reached this way" may sort differently than globally.
```
User {
  posts: Post[] (Post.author) @sort(pinned desc, created_at desc)
}
```

## Query override (most specific)
```
query recent(user -> author) -> PostShape[] order (updated_at desc);
```

## Lint
A `list` with no sort at any tier returns nondeterministic order -> warn ("results nondeterministic; add @sort or order"). Same cheap-lint-prevents-prod-surprise pattern as `unindexed`. (Keyset pagination already forces a sort; this mainly catches non-paginated lists.)
