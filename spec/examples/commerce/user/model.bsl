@soft_delete(deleted_at)
User {
  id:            Id
  deleted_at:    timestamp?
  email:         text (unique)
  name:          text
  invited_by:    User?
  invited_users: User[]  (User.invited_by)
  placed_orders: Order[] (Order.placed_by)
}

# The shared User projection: any shape may nest it by name (`placed_by -> UserRef`),
# so every fetch site and mapper works against this one nominal type.
shape UserRef from User { name, email }
