# Whole-query raw bodies: a clean typed-param body checks; an untyped param is
# E0210, `${ctx.…}` is E0214, `scoped` is E0211, and the soft-delete gap on the
# target model is the W0102 lint.
@soft_delete(deleted_at)
User { deleted_at: timestamp?, name: text, email: text, total: int }
shape UserRow from User { name, email }

query heavy_users(min: int) -> UserRow[] {
  raw`SELECT u.name AS name, u.email AS email FROM user u WHERE u.total >= ${min} AND u.deleted_at IS NULL`;
}

query untyped(min) -> UserRow[] {
  raw`SELECT name, email FROM user WHERE total >= ${min}`;
}

query ctx_leak() -> UserRow[] {
  raw`SELECT name, email FROM user WHERE org = ${ctx.org}`;
}
