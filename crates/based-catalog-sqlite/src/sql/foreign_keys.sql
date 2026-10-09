SELECT id, seq, "table" AS target, "from" AS local_column, "to" AS target_column, on_update, on_delete, "match" AS match_mode
FROM pragma_foreign_key_list(?, 'main') ORDER BY id, seq
