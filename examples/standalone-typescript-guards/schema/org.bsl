Org { id: Id name: text }
mutation create_org(name) -> Org { create Org { name = $name }; }
