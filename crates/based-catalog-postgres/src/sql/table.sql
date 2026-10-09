SELECT c.oid::bigint AS oid, c.relkind::text AS kind, c.relrowsecurity, c.relforcerowsecurity,
       c.relispartition, c.reloptions, c.relchecks, c.relpersistence::text AS persistence,
       EXISTS(SELECT 1 FROM pg_catalog.pg_inherits inh WHERE inh.inhrelid = c.oid OR inh.inhparent = c.oid) AS inherits,
       pg_catalog.has_schema_privilege(n.oid, 'USAGE') AS visible
FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r','p','v','m','f')
