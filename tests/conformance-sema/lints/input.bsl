# Lint surface: a `list` with no sort at any tier (W0100 nondeterministic), a
# declared index nothing uses (W0104 useless), and an unknown decorator (W0101).
@frobnicate
Widget {
  id: Id
  name:  text
  color: text
  @index(color)
}

shape WidgetCard from Widget { name }

query widgets() -> WidgetCard[];
