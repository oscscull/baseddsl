# The `-> ok` / read-back rules. `-> ok` is the universal opt-out of read-back (BW1),
# so a surviving write under it is legal (`remove_comment`). Still errors: a shape on a
# real DELETE is E0220, `ok` on a query is E0222, and a raw-only `-> ok` (no engine-known
# write to hang scope/sharding on) is E0221.
Tag { id: Id, label: text }
shape TagCard from Tag { label }

@soft_delete(deleted_at)
Comment { id: Id, deleted_at: timestamp?, body: text }

mutation drop_tag(id: Id) -> TagCard {
  delete Tag where (id = $id);
}

# Legal since BW1: a surviving write (soft-delete tombstone) may opt out of read-back.
mutation remove_comment(id: Id) -> ok {
  delete Comment where (id = $id);
}

# Raw-only `-> ok`: no engine-known write to make primary -> E0221.
mutation vacuum() -> ok {
  raw`VACUUM`;
}

query tags() -> ok;
