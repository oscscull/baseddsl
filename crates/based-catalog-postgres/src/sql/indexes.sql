SELECT ic.relname::text AS name, i.indisunique, i.indisprimary, i.indnkeyatts::int AS key_count,
       i.indisvalid AND i.indisready AND i.indislive AS valid, i.indnullsnotdistinct,
       i.indisexclusion, i.indisclustered, i.indisreplident, ic.reloptions,
       ic.reltablespace <> 0 AS custom_tablespace, am.amname::text AS method,
       EXISTS(SELECT 1 FROM pg_catalog.pg_constraint c WHERE c.conindid = i.indexrelid AND c.contype = 'u') AS unique_constraint,
       part.position::int AS position, a.attname::text AS column_name,
       pg_catalog.pg_get_indexdef(i.indexrelid,part.position::int,false) AS part_definition,
       pg_catalog.pg_get_indexdef(i.indexrelid,0,false) AS definition,
       pg_catalog.pg_get_expr(i.indpred,i.indrelid,false) AS predicate,
       pg_catalog.pg_index_column_has_property(i.indexrelid,part.position::int,'desc') AS descending,
       pg_catalog.pg_index_column_has_property(i.indexrelid,part.position::int,'nulls_first') AS nulls_first,
       opc.opcdefault,
       CASE WHEN col.oid IS NULL THEN NULL ELSE pg_catalog.format('%I.%I',cn.nspname,col.collname) END AS collation
FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
JOIN pg_catalog.pg_am am ON am.oid = ic.relam
JOIN LATERAL unnest(i.indkey) WITH ORDINALITY AS part(number,position) ON true
LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = part.number
LEFT JOIN pg_catalog.pg_opclass opc ON opc.oid = i.indclass[part.position::int - 1]
LEFT JOIN pg_catalog.pg_collation col ON col.oid = i.indcollation[part.position::int - 1]
LEFT JOIN pg_catalog.pg_namespace cn ON cn.oid = col.collnamespace
WHERE i.indrelid = $1::bigint::oid ORDER BY ic.relname, part.position
