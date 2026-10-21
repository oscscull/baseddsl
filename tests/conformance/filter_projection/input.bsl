filter overdue(before) = status = waiting and since <= $before;
filter active = done = false;

shape TaskCard from Task {
  id
  is_overdue = overdue($before)
  is_active  = active()
  total      = count()
}
