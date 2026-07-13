# Desk staffing. Plain model, so no scope acknowledgement — the caller names
# the org outright (onboarding runs before any session context exists).
mutation create_user(org: Id, name: text, email: text, role: Role, rate: decimal(8, 2)?) -> UserRow {
  create User { org = $org, name = $name, email = $email, role = $role, rate = $rate };
}
