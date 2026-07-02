# A grab-bag of resolution errors: unknown return type (E0140), get keyed on a
# non-unique field (E0144), an unknown field in a shape (E0111), and a duplicate
# model (E0100). Confirms the diagnostics render stably and codegen is skipped.
User {
  name:  text
  email: text (unique)
}

User {
  name: text
}

shape UserCard from User { name, handle }

query user_by_name(name) -> UserCard;
query user_by_email(email) -> Missing;
