Item {
  id: Id
  name: text
}

mutation create_item(name: text) -> Item {
  create Item { name = $name };
}

query items() -> Item[] {
  list Item order (id);
}
