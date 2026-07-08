@soft_delete(deleted_at)
User {
  deleted_at: timestamp?
  email:      text (unique)
  name:       text
}

shape UserRow from User { id, email, name }

# Seed a user (public, like create_org).
mutation create_user(email, name) -> UserRow {
  create User { email = $email, name = $name };
}
