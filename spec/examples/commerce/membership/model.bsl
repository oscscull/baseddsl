# join model carrying role
@soft_delete(deleted_at)
Membership {
  deleted_at: timestamp?
  org:        Org
  user:       User
  role:       text (default "member")
  @index(org, user) unique
}
