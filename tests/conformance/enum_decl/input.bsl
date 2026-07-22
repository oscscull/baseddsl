enum Status { pending, paid = "PAID", shipped, cancelled }

enum Priority { low = 0, medium = 1, high = 2 }

Order {
  id: Id
  status:   Status (default pending)
  priority: Priority (default low)
  total:    int
}
