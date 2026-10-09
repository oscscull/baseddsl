SELECT a.attname::text AS name, a.attnum::int AS position, (a.attnotnull OR t.typnotnull) AS attnotnull, a.atttypmod,
       a.attidentity::text AS identity, a.attgenerated::text AS generated,
       pg_catalog.format_type(a.atttypid, a.atttypmod) AS declaration,
       t.oid::bigint AS type_oid, t.typname::text AS type_name, t.typtype::text AS type_kind,
       t.typcategory::text AS category, tn.nspname::text AS type_namespace,
       pg_catalog.pg_get_expr(d.adbin,d.adrelid,false) AS expression,
       CASE WHEN col.oid IS NULL THEN NULL ELSE pg_catalog.format('%I.%I',cn.nspname,col.collname) END AS collation,
       a.attoptions, a.attfdwoptions
FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type t ON t.oid = a.atttypid
JOIN pg_catalog.pg_namespace tn ON tn.oid = t.typnamespace
LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
LEFT JOIN pg_catalog.pg_collation col ON col.oid = a.attcollation
LEFT JOIN pg_catalog.pg_namespace cn ON cn.oid = col.collnamespace
WHERE a.attrelid = $1::bigint::oid AND a.attnum > 0 AND NOT a.attisdropped
ORDER BY a.attnum
