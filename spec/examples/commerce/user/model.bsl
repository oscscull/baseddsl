@soft_delete(deleted_at)
User {
  deleted_at:    timestamp?
  email:         text (unique)
  name:          text
  invited_by:    User?
  invited_users: User[]  (User.invited_by)
  placed_orders: Order[] (Order.placed_by)
}
