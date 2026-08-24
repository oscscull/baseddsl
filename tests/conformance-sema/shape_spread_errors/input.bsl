User {
  id:    Id
  name:  text
  email: text
}

Org {
  id:    Id
  title: text
}

shape UserBase from User {
  id
  name
}

shape OrgBase from Org {
  id
  title
}

# E0135 — unknown spread target
shape S1 from User { ...Nope }

# E0136 — cross-model spread (OrgBase is from Org, not User)
shape S2 from User { ...OrgBase }

# E0138 — duplicate field after composition (name is spliced and local)
shape S3 from User { ...UserBase, name }

# E0137 — spread cycle
shape A from User { ...B }
shape B from User { ...A }

# E0139 — spread inside a nest (not top level)
Post {
  id:     Id
  title:  text
  author: User
}
shape PostBad from Post { title, author { ...UserBase } }
