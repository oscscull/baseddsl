SELECT name, "unique" AS is_unique, origin, partial
FROM pragma_index_list(?, 'main') ORDER BY name
