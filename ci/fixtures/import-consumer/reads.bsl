shape LegacyRow from LegacyEntry { heading parent_code { label } }
query imported_entries() -> LegacyRow[] { list LegacyEntry order (entry_key asc); }
