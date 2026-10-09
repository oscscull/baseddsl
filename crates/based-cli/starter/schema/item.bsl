scope Author (owner: uuid = $ctx.owner)

@scope Author
Item {
  id: Id
  name: text
  owner: uuid
  parent: Item?
  @index owner
}

shape ItemView from Item {
  id
  name
  parent { id, name }
}

mutation create_item(name: text, parent: Item?) -> ItemView scoped Author {
  create Item { name = $name, parent = $parent };
}

query items() -> ItemView[] scoped Author {
  list Item order (id);
}

query item_by_id(id) -> ItemView scoped Author;
