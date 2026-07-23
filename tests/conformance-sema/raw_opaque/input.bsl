# Opaque `raw(…)` columns + the two exotic index tiers.

Place {
  id:       Id
  name:     text
  location: raw("geometry(Point,4326)")?
  search:   raw({ postgres: "tsvector", mariadb: "text", sqlite: "text" })?

  @index name
  @index location using gist
  @index raw("(lower(name))")
}

shape PlaceRow from Place {
  id
  name
  location
  area = raw`ST_Area(location)`
}

query place(id) -> PlaceRow;

query by_name(name) -> PlaceRow[] order (name);

# --- rejected uses -------------------------------------------------------
Bad {
  id:    Id
  blob:  raw("bytea")
  other: raw("inet")?

  @index blob using nope
  @index raw("")
}

shape BadRow from Bad { id }

query bad_filter() -> BadRow[] {
  list Bad where (other = "1.2.3.4") order (other);
}

mutation add_bad(v) -> BadRow {
  create Bad { other = $v }
}
