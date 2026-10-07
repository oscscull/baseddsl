Owner { id: Id, name: text, items: Item[] }
@sort(name asc)
Item { id: Id, owner: Owner?, name: text, payload: json?, wide: decimal(38,9), fraction: decimal(38,38), @index(owner), @index(name) }
shape ItemRow from Item { id, name, payload, wide, fraction }
shape OwnerRow from Owner { id, name, items -> ItemRow }
mutation create_owner(name: text) -> OwnerRow { create Owner { name = $name }; }
mutation create_item(owner: Id?, name: text, payload: json?, wide: decimal(38,9), fraction: decimal(38,38)) -> ItemRow {
 create Item { owner = $owner, name = $name, payload = $payload, wide = $wide, fraction = $fraction };
}
mutation remove_item(id: Id) -> ok { hard delete Item where (id = $id); }
query item_by_id(id) -> ItemRow;
query owner_by_id(id) -> OwnerRow;
query find_items(names: text[]) -> ItemRow[] { list Item where (name in $names) order (name); }
query item_page() -> ItemRow[] { list Item order (name) page (1) offset with count; }
query export_items() -> stream ItemRow { list Item order (name); }
