scope Author (owner: uuid = $ctx.owner)

@scope Author
Item {
  id: Id
  title: text @was("name")
  owner: uuid
  parent: Item?
  @index owner
}

shape ItemView from Item {
  id
  name = title
  parent { id, name = title }
}

mutation create_item(name: text, parent: Item?) -> ItemView scoped Author {
  create Item { title = $name, parent = $parent };
}

query items() -> ItemView[] scoped Author {
  list Item order (id);
}

query item_by_id(id) -> ItemView scoped Author;
