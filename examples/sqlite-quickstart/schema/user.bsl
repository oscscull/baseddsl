@soft_delete(deleted_at)
User {
  deleted_at: timestamp?
  email:      text (unique)
  name:       text
}
