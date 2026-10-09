SELECT cid, name, type, "notnull" AS required, dflt_value, pk, hidden
FROM pragma_table_xinfo(?, 'main') ORDER BY cid
