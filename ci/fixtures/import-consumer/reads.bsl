shape LegacyRow from LegacyEntry { heading parent_code { label } }
query imported_entries() -> LegacyRow[] { list LegacyEntry order (entry_key asc); }
shape PostingRow from LegacyPosting { memo legacy_ledger { region ledger_no title } }
query imported_postings() -> PostingRow[] { list LegacyPosting order (posting_key asc); }
