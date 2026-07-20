# `-> ok`: the shapeless acknowledgement of a destructive mutation.
Tag { label: text }

mutation drop_tag(id: Id) -> ok {
  delete Tag where (id = $id);
}
