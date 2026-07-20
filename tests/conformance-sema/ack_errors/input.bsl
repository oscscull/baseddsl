# The `-> ok` / real-DELETE pairing rules: a shape on a real DELETE is E0220, an
# ack on a surviving write (here a soft delete) is E0221, and `ok` on a query is
# E0222.
Tag { label: text }
shape TagCard from Tag { label }

@soft_delete(deleted_at)
Comment { deleted_at: timestamp?, body: text }

mutation drop_tag(id: Id) -> TagCard {
  delete Tag where (id = $id);
}

mutation remove_comment(id: Id) -> ok {
  delete Comment where (id = $id);
}

query tags() -> ok;
