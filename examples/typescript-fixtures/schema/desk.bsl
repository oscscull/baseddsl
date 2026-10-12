scope Tenant (org: Org = $ctx.org)

enum Role { requester, agent }

Org {
  id:   Id
  name: text
  slug: text (unique)
}

User {
  id:     Id
  org:    Org
  email:  text (unique)
  role:   Role
  active: bool (default true)
}

@created(created_at)
@scope Tenant
Ticket {
  id:         Id
  org:        Org
  requester:  User
  assignee:   User?
  subject:    text
  estimate:   decimal(6, 2)
  due:        date?
  created_at: timestamp
}
