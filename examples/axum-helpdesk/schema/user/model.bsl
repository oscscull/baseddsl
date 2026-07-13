# One person on the desk — agent or requester, told apart by `role`.

# The two user-keyed scopes. Requester confines Ticket rows to the person who
# opened them; Author confines DraftNote rows to the agent who wrote them. Both
# bind the same `$ctx.user`, so one session context serves either audience.
scope Requester (requester: User = $ctx.user)
scope Author (author: User = $ctx.user)

enum Role { requester, agent, admin }

@sort(name)
User {
  org:   Org
  name:  text
  email: text (unique)
  role:  Role (default requester)
  rate:  decimal(8, 2)?
  @index(org, role)
}

# The shared person projection: any shape nests it by name (`author -> UserRef`),
# so every call site works against this one nominal type.
shape UserRef from User { id, name, email }

shape UserRow from User {
  id
  name
  email
  role
}
