# slug carries no @index: `(unique)` already backs it with a unique key, so a
# plain index on it would be flagged W0104 (pure write tax).
@soft_delete(deleted_at)
Org {
  deleted_at: timestamp?
  name:       text
  slug:       text (unique)
}
