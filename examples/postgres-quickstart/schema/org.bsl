@soft_delete(deleted_at)
Org {
  deleted_at: timestamp?
  name:       text
  slug:       text (unique)
}
