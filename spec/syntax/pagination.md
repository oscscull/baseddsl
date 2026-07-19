# syntax/pagination.md

Principles: 1 (keyset default, offset opt-in), 8 (tiebreaker shown).

## Keyset default; offset explicit
Offset degrades at depth + is incorrect under concurrent writes = the dangerous one -> visible opt-in.
```
list User where (active) order (created_at desc) page (20);   # keyset: 20 live rows + cursor
list User order (id) page (50) offset;                        # explicit offset
list User order (id) page (50) with count;                    # opt into total
```

## Rules
- `order (field)` is the cursor basis. Falls back to model/relation `@sort` if the query gives none (sorting.md).
- Engine auto-appends a unique tiebreaker (id) when sort key isn't unique (else keyset drops/repeats rows). Shown, not written.
- Cursor is opaque, engine-derived, validated/signed (no predicate injection). User never assembles keyset mechanics.
- Total count opt-in (`with count`): second expensive query, meaningless for keyset. Default = page + "more" cursor, no total. The wire carries `total` only for a `with count` query; the client `Page` surfaces it as an optional field, populated exactly then (calling.md). Count queries also subject to index lint.
- Page size counts live rows (soft-delete filter applied before limit).

## Pagination vs streaming
Pagination is random access (a bounded page + a re-entry cursor); a `-> stream` query is
the one unbounded forward pass (exports, large scans). They never combine — `page` on a
stream query is a compile error. See streaming.md.
