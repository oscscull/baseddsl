use super::*;

/// Reassemble a flat result row into the response object, nesting sub-objects/arrays.
///
/// A nested to-one shape sub-object (`buyer { name, email }`) is projected by codegen
/// as columns aliased `buyer.name`, `buyer.email` ([`based_codegen::sql::NEST_SEP`] is
/// the `.`); this splits each such key back into a nested object, recursing for
/// nested-within-nested (`buyer.org.name`). A to-many nest (`items { … }`) is projected as
/// a single JSON-array string column aliased `items[]`
/// ([`based_codegen::sql::ARRAY_MARK`]); this parses the string into a real JSON array of
/// sub-objects (their own nesting already fully formed by the SQL JSON aggregation). A
/// `.`/`[`/`]` cannot occur in a BSL identifier, so a flat query (no nest) has no such key
/// and passes through unchanged.
pub(crate) fn nest_row(row: Row) -> serde_json::Value {
    let mut root = serde_json::Map::new();
    for (key, val) in row {
        insert_path(&mut root, &key, val);
    }
    let mut value = serde_json::Value::Object(root);
    collapse_absent_nests(&mut value);
    value
}

/// Collapse absent to-one nests. A LEFT-JOINed nest projects a presence probe
/// (`<field>.__present` = the child's `id`, [`based_codegen::sql::NEST_PRESENT`]):
/// a NULL probe means the joined row does not exist, so the whole sub-object —
/// otherwise an indistinguishable object of NULLs — becomes JSON null. A matched
/// row just sheds the probe. Recurses for nests within nests.
pub(crate) fn collapse_absent_nests(value: &mut serde_json::Value) {
    use serde_json::Value as J;
    if let J::Object(map) = value {
        if let Some(probe) = map.remove(based_codegen::sql::NEST_PRESENT) {
            if probe.is_null() {
                *value = J::Null;
                return;
            }
        }
        for child in map.values_mut() {
            collapse_absent_nests(child);
        }
    }
}

/// Parse a to-many array column's value into a JSON array. The DB returns the aggregated
/// column as a JSON-array *string* (SQLite/MariaDB text); a driver that decodes the JSON
/// type natively hands back an array already, and an empty group may arrive as NULL — all
/// three normalize to an array here (a malformed string, which the engine never emits,
/// degrades to `[]` rather than panicking).
pub(crate) fn parse_array(val: serde_json::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match val {
        J::String(s) => serde_json::from_str(&s).unwrap_or(J::Array(Vec::new())),
        arr @ J::Array(_) => arr,
        _ => J::Array(Vec::new()),
    }
}

/// Normalize every `json`-typed leaf named by `paths` in one already-nested result row.
/// A `json` column stores JSON as text (SQLite/MariaDB), so a driver reads it back as a
/// JSON *string*; parsing it here yields the structured object/array it holds, so a `json`
/// field round-trips as what was written, not a double-encoded string. A value already
/// structured (a driver that decodes json natively) is left untouched, and a string that
/// does not parse as JSON is left as-is (never a panic).
pub(crate) fn normalize_json(row: &mut serde_json::Value, paths: &[String]) {
    for path in paths {
        let segs: Vec<&str> = path.split(based_codegen::sql::NEST_SEP).collect();
        parse_json_at(row, &segs);
    }
}

/// Descend `value` along `segs` (a `.`-split json path; a `field[]` segment is an array to
/// recurse into per element) and, at the leaf, parse a JSON string into structured JSON.
pub(crate) fn parse_json_at(value: &mut serde_json::Value, segs: &[&str]) {
    use serde_json::Value as J;
    let Some((head, rest)) = segs.split_first() else {
        if let J::String(s) = value {
            if let Ok(parsed) = serde_json::from_str::<J>(s) {
                *value = parsed;
            }
        }
        return;
    };
    match head.strip_suffix(ARRAY_MARK) {
        Some(key) => {
            if let J::Object(map) = value {
                if let Some(J::Array(arr)) = map.get_mut(key) {
                    for elem in arr.iter_mut() {
                        parse_json_at(elem, rest);
                    }
                }
            }
        }
        None => {
            if let J::Object(map) = value {
                if let Some(child) = map.get_mut(*head) {
                    parse_json_at(child, rest);
                }
            }
        }
    }
}

/// Insert `val` at a possibly-dotted `key` into `obj`, creating intermediate objects for
/// each `NEST_SEP` segment (`buyer.org.name` → `{buyer:{org:{name:val}}}`). A leaf key
/// suffixed with `ARRAY_MARK` (`items[]`) is a to-many array: its string value is parsed
/// into a JSON array and stored under the field name without the marker.
pub(crate) fn insert_path(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    val: serde_json::Value,
) {
    match key.split_once(based_codegen::sql::NEST_SEP) {
        None => match key.strip_suffix(ARRAY_MARK) {
            Some(name) => {
                obj.insert(name.to_string(), parse_array(val));
            }
            None => {
                obj.insert(key.to_string(), val);
            }
        },
        Some((head, rest)) => {
            let entry = obj
                .entry(head.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let serde_json::Value::Object(child) = entry {
                insert_path(child, rest, val);
            }
        }
    }
}

/// Mint the "more" cursor for a keyset page: the last row's sort-key values, read from the
/// hidden `__keyset_<i>` columns codegen projected. Only a full page (`page_size` rows) can
/// have a next page — a short page is the last, so it gets no cursor (the caller stops
/// paging rather than making one more empty request).
pub(crate) fn next_cursor(rows: &[Row], ks: KeysetPlan) -> Option<String> {
    use serde_json::Value as J;
    if (rows.len() as u64) < ks.page_size {
        return None;
    }
    let last = rows.last()?;
    let vals: Vec<J> = (0..ks.keys)
        .map(|i| {
            last.get(&format!("{KEYSET_PREFIX}{i}"))
                .cloned()
                .unwrap_or(J::Null)
        })
        .collect();
    Some(crate::cursor::encode(&vals))
}

/// Execute a plan's statements and assemble the response per its envelope.
pub(crate) async fn shape<D: DbRead + ?Sized>(
    db: &mut D,
    plan: &QueryPlan,
) -> Result<serde_json::Value, DbError> {
    use serde_json::Value as J;
    let mut rows = fetch_all(db.fetch(&plan.main.sql, &plan.main.params)).await?;
    // Nest a flat row into the response object, then normalize its `json` leaves (a json
    // column read back as a text string → structured JSON).
    let paths = &plan.json_paths;
    let nest = |row: Row| {
        let mut v = nest_row(row);
        if !paths.is_empty() {
            normalize_json(&mut v, paths);
        }
        v
    };
    Ok(match plan.envelope {
        // `get`: the first row, or JSON null (Option<T>).
        Envelope::One => rows.into_iter().next().map_or(J::Null, nest),
        // `list`: every row as an array.
        Envelope::Many => J::Array(rows.into_iter().map(nest).collect()),
        // paginated `list`: the { rows, cursor } envelope. For a keyset page, mint the
        // next cursor from the last row's hidden sort-key columns and strip them from
        // the response; `total` rides along when the query asked for a count.
        Envelope::Page { with_count } => {
            let cursor = plan.keyset.and_then(|ks| next_cursor(&rows, ks));
            if plan.keyset.is_some() {
                for r in &mut rows {
                    r.retain(|k, _| !k.starts_with(KEYSET_PREFIX));
                }
            }
            let mut obj = serde_json::Map::new();
            obj.insert("rows".into(), J::Array(rows.into_iter().map(nest).collect()));
            obj.insert("cursor".into(), cursor.map_or(J::Null, J::String));
            if with_count {
                if let Some(count) = &plan.count {
                    let total = fetch_all(db.fetch(&count.sql, &count.params))
                        .await?
                        .into_iter()
                        .next()
                        .and_then(|mut r| r.remove("count"))
                        .unwrap_or(J::Null);
                    obj.insert("total".into(), total);
                }
            }
            J::Object(obj)
        }
    })
}
