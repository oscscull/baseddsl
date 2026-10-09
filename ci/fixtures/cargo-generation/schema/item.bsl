Item { id: serial, name: text }
shape ItemRow from Item { id, name }
query items() -> ItemRow[] { list Item; }
