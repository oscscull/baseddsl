# Whole-query raw body (raw.md's third level): the block is one `sql` backtick
# statement. `${param}` interpolations stay bound parameters; the declared shape
# types the result columns.
User { name: text, email: text, total: int }
shape UserRow from User { name, email }

query heavy_users(min: int) -> UserRow[] {
  sql`SELECT u.name AS name, u.email AS email
      FROM user u
      WHERE u.total >= ${min}`;
}
