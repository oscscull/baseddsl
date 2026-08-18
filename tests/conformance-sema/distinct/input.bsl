# `list distinct` — dedup projected rows (SELECT DISTINCT). A `list`-only modifier;
# the automatic sort cascade is suppressed so an injected key column can't defeat the
# dedup. Diagnostics: E0310 (keyset page), E0311 (aggregate query), E0312 (order column
# not projected), W0111 (projection carries the primary key — dedup is a no-op).
City {
  id:     Id
  name:   text
  region: text
}

shape RegionName from City {
  region
}

# Clean: projects only `region`, ordered by the projected column.
query regions() -> RegionName[] {
  list distinct City order (region);
}

shape RegionCount from City {
  region
  n = count()
}

# E0311: `distinct` on an aggregate query is redundant.
query counts() -> RegionCount[] {
  list distinct City group by (region);
}

# E0310: `distinct` with a keyset `page`.
query paged() -> RegionName[] {
  list distinct City order (region) page (20);
}

# E0312: orders by `name`, which the shape does not project.
query bad_order() -> RegionName[] {
  list distinct City order (name);
}

# W0111: a bare-model return projects the primary key, so dedup is a no-op.
query all() -> City[] {
  list distinct City order (name);
}
