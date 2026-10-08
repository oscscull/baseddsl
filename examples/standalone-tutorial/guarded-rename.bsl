mutation rename_item(id: Id, name: text, expected_name: text) -> ItemView guard caller_can_rename scoped Author {
  update Item where (id = $id and name = $expected_name) { name = $name };
}
