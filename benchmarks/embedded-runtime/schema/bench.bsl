@sort(id asc)
Owner { id: serial, name: text, items: Item[] }
@sort(id asc)
Item { id: serial, owner: Owner, name: text, value: int, @index(owner) }
shape ItemRow from Item { id, name, value }
shape OwnerRow from Owner { id, name, items -> ItemRow }
shape ItemInput from Item { owner { id }, name, value }
query flat(size: int) -> ItemRow[] { list Item where (id <= $size) order (id); }
query nested(size: int) -> OwnerRow[] { list Owner where (id <= $size) order (id); }
query paged() -> ItemRow[] { list Item order (id) page (32) offset with count; }
mutation bulk(rows: ItemInput[]) -> ok { create Item[] from $rows; }
