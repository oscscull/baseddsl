enum TaskStatus { waiting, done }
User { id: Id, name: text }
Task {
  id:     Id
  status: TaskStatus
  since:  timestamp
  owner:  User
  @index status
}

filter overdue(before) = status = waiting and since <= $before;
filter mine = owner = $ctx.user;

shape TaskCard from Task {
  id
  is_overdue = overdue($before)
  is_mine    = mine()
}

# Binds `$before` and requires `$ctx.user` through the projected `mine`.
query due(before: timestamp) -> TaskCard[] { list Task where (overdue($before)); }

# No `before` param: E0355.
query board() -> TaskCard[] { list Task; }

# `before` disagrees with the filter's timestamp column: E0355.
query wrong(before: int) -> TaskCard[] { list Task; }
