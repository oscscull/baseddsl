SELECT c.conname::text AS name, c.contype::text AS kind,
       c.condeferrable, c.condeferred, a.attname::text AS column_name
FROM pg_catalog.pg_constraint c
JOIN LATERAL unnest(c.conkey) WITH ORDINALITY AS part(number,position) ON true
JOIN pg_catalog.pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = part.number
WHERE c.conrelid = $1::bigint::oid AND c.contype IN ('p','u')
ORDER BY c.conname, part.position
