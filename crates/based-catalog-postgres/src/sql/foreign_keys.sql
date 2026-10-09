SELECT c.conname::text AS name, c.condeferrable, c.condeferred,
       c.confdeltype::text AS delete_action, c.confupdtype::text AS update_action,
       c.confmatchtype::text AS match_mode, a.attname::text AS column_name,
       tn.nspname::text AS target_namespace, tc.relname::text AS target_name,
       ta.attname::text AS target_column
FROM pg_catalog.pg_constraint c
JOIN pg_catalog.pg_class tc ON tc.oid = c.confrelid
JOIN pg_catalog.pg_namespace tn ON tn.oid = tc.relnamespace
JOIN LATERAL unnest(c.conkey,c.confkey) WITH ORDINALITY AS part(local_number,target_number,position) ON true
JOIN pg_catalog.pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = part.local_number
JOIN pg_catalog.pg_attribute ta ON ta.attrelid = c.confrelid AND ta.attnum = part.target_number
WHERE c.conrelid = $1::bigint::oid AND c.contype = 'f'
ORDER BY c.conname, part.position
