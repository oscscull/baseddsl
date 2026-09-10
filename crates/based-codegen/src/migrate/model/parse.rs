//! Parse a `schema.snap` back into the neutral [`Snapshot`] — the inverse of
//! [`Snapshot::render`] — so a stored baseline can be diffed against the current schema.

use super::*;

/// A `schema.snap` that could not be parsed — the file is corrupt or hand-edited into
/// an invalid state. Carries the 1-based line and a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "schema.snap line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

impl Snapshot {
    /// Parse a `schema.snap` back into the neutral model — the inverse of [`render`], so
    /// the stored baseline can be diffed against the current schema. Whitespace-tolerant
    /// on the leading indent; comments (`#…`) and the header line are skipped.
    ///
    /// [`render`]: Snapshot::render
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let mut scopes: Vec<ScopeDeclSnap> = Vec::new();
        let mut tables: Vec<TableSnap> = Vec::new();
        let mut renames: Vec<Rename> = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line_no = i + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("snapshot ") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("scope ") {
                scopes.push(parse_scope_decl(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("rename ") {
                renames.push(parse_rename(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("table ") {
                tables.push(parse_table_header(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("column ") {
                let t = tables.last_mut().ok_or_else(|| ParseError {
                    line: line_no,
                    message: "column before any table".to_string(),
                })?;
                t.columns.push(parse_column(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("index ") {
                let t = tables.last_mut().ok_or_else(|| ParseError {
                    line: line_no,
                    message: "index before any table".to_string(),
                })?;
                t.indexes.push(parse_index(rest, line_no)?);
            } else if let Some(rest) = line.strip_prefix("fk ") {
                let t = tables.last_mut().ok_or_else(|| ParseError {
                    line: line_no,
                    message: "fk before any table".to_string(),
                })?;
                t.foreign_keys.push(parse_fk(rest, line_no)?);
            } else {
                return Err(ParseError {
                    line: line_no,
                    message: format!("unrecognized line: {line}"),
                });
            }
        }
        Ok(Self {
            scopes,
            tables,
            renames,
        })
    }
}

/// Parse a `table <old> -> <new>` / `column <table>.<old> -> <new>` rename line (the
/// `rename ` prefix already stripped).
fn parse_rename(rest: &str, line: usize) -> Result<Rename, ParseError> {
    let malformed = || ParseError {
        line,
        message: format!("malformed rename: {rest}"),
    };
    if let Some(spec) = rest.strip_prefix("table ") {
        let (from, to) = spec.split_once("->").ok_or_else(malformed)?;
        Ok(Rename::Table {
            from: from.trim().to_string(),
            to: to.trim().to_string(),
        })
    } else if let Some(spec) = rest.strip_prefix("column ") {
        let (lhs, to) = spec.split_once("->").ok_or_else(malformed)?;
        let (table, from) = lhs.trim().split_once('.').ok_or_else(malformed)?;
        Ok(Rename::Column {
            table: table.trim().to_string(),
            from: from.trim().to_string(),
            to: to.trim().to_string(),
        })
    } else {
        Err(malformed())
    }
}

/// Parse a top-level `scope Name (col: Type = $ctx.field, …)` decl line.
fn parse_scope_decl(rest: &str, line: usize) -> Result<ScopeDeclSnap, ParseError> {
    let open = rest.find('(').ok_or_else(|| ParseError {
        line,
        message: format!("scope decl has no term list: {rest}"),
    })?;
    let name = rest[..open].trim().to_string();
    if name.is_empty() {
        return Err(ParseError {
            line,
            message: "scope decl has no name".to_string(),
        });
    }
    let close = rest.rfind(')').ok_or_else(|| ParseError {
        line,
        message: format!("scope decl term list is not closed: {rest}"),
    })?;
    let inner = rest[open + 1..close].trim();
    let mut terms = Vec::new();
    if !inner.is_empty() {
        for t in inner.split(',') {
            let (col_ty, ctx) = t.split_once(" = $ctx.").ok_or_else(|| ParseError {
                line,
                message: format!("malformed scope term: {t}"),
            })?;
            let (col, ty) = col_ty.split_once(':').ok_or_else(|| ParseError {
                line,
                message: format!("scope term has no type: {t}"),
            })?;
            terms.push(ScopeTermSnap {
                column: col.trim().to_string(),
                ty: ty.trim().to_string(),
                ctx_field: ctx.trim().to_string(),
            });
        }
    }
    Ok(ScopeDeclSnap { name, terms })
}

/// Split off a `key=(a, b, c)` group after `key=`, returning the inner terms and the
/// remaining tail. Returns `None` when `head` does not start with `key=(`.
fn take_group<'a>(head: &'a str, key: &str) -> Option<(Vec<String>, &'a str)> {
    let after = head.strip_prefix(key)?.strip_prefix("=(")?;
    let close = after.find(')')?;
    let inner = &after[..close];
    let tail = after[close + 1..].trim_start();
    let terms = if inner.trim().is_empty() {
        Vec::new()
    } else {
        inner.split(',').map(|s| s.trim().to_string()).collect()
    };
    Some((terms, tail))
}

/// Split the `pk=` value off `head`: a bare `pk=col` (single) or a `pk=(c1, c2)` tuple
/// (composite — the parenthesized list carries spaces). Returns the marker text and the
/// remaining tail.
fn split_pk_marker(after: &str) -> (&str, &str) {
    if after.starts_with('(') {
        let close = after.find(')').map_or(after.len(), |i| i + 1);
        (&after[..close], after[close..].trim_start())
    } else {
        let mut sp = after.splitn(2, char::is_whitespace);
        (sp.next().unwrap_or(""), sp.next().unwrap_or("").trim_start())
    }
}

/// Parse a `sort=(col dir, …)` group's terms into `(column, dir)` pairs.
fn parse_sort_terms(terms: Vec<String>, line: usize) -> Result<Vec<(String, String)>, ParseError> {
    let mut sort = Vec::new();
    for t in terms {
        let (col, dir) = t
            .split_once(char::is_whitespace)
            .ok_or_else(|| ParseError {
                line,
                message: format!("malformed sort term: {t}"),
            })?;
        sort.push((col.trim().to_string(), dir.trim().to_string()));
    }
    Ok(sort)
}

fn parse_table_header(rest: &str, line: usize) -> Result<TableSnap, ParseError> {
    let mut it = rest.splitn(2, char::is_whitespace);
    let name = it.next().unwrap_or("").to_string();
    if name.is_empty() {
        return Err(ParseError {
            line,
            message: "table has no name".to_string(),
        });
    }
    let mut head = it.next().unwrap_or("").trim_start();

    let mut soft_delete = None;
    let mut created = None;
    let mut updated = None;
    let mut scope_alts = Vec::new();
    let mut sort = Vec::new();
    let mut no_id = false;
    let mut pk: Vec<String> = Vec::new();
    let mut schema = None;

    while !head.is_empty() {
        if head == "no_id" || head.starts_with("no_id ") {
            no_id = true;
            head = head["no_id".len()..].trim_start();
        } else if let Some(after) = head.strip_prefix("schema=") {
            let mut sp = after.splitn(2, char::is_whitespace);
            schema = Some(sp.next().unwrap_or("").to_string());
            head = sp.next().unwrap_or("").trim_start();
        } else if let Some(after) = head.strip_prefix("pk=") {
            let (marker, rest) = split_pk_marker(after);
            pk = parse_col_list(marker);
            head = rest;
        } else if let Some(after) = head.strip_prefix("soft_delete=") {
            let mut sp = after.splitn(2, char::is_whitespace);
            let spec = sp.next().unwrap_or("");
            let (col, mode) = spec.split_once(':').ok_or_else(|| ParseError {
                line,
                message: format!("malformed soft_delete: {spec}"),
            })?;
            soft_delete = Some((col.to_string(), mode.to_string()));
            head = sp.next().unwrap_or("").trim_start();
        } else if let Some(after) = head.strip_prefix("created=") {
            let mut sp = after.splitn(2, char::is_whitespace);
            created = Some(sp.next().unwrap_or("").to_string());
            head = sp.next().unwrap_or("").trim_start();
        } else if let Some(after) = head.strip_prefix("updated=") {
            let mut sp = after.splitn(2, char::is_whitespace);
            updated = Some(sp.next().unwrap_or("").to_string());
            head = sp.next().unwrap_or("").trim_start();
        } else if let Some((names, tail)) = take_group(head, "scope") {
            // Each `scope=(A, B)` group is one `@scope` alternative (a name set).
            scope_alts.push(names);
            head = tail;
        } else if let Some((terms, tail)) = take_group(head, "sort") {
            sort = parse_sort_terms(terms, line)?;
            head = tail;
        } else {
            return Err(ParseError {
                line,
                message: format!("unrecognized table attribute: {head}"),
            });
        }
    }

    Ok(TableSnap {
        name,
        schema,
        soft_delete,
        created,
        updated,
        scope_alts,
        sort,
        no_id,
        pk,
        columns: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
    })
}

/// Parse an FK column list — a bare name, or a `(c1, c2)` parenthesized tuple.
fn parse_col_list(s: &str) -> Vec<String> {
    let s = s.trim();
    match s.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        Some(inner) => inner.split(',').map(|c| c.trim().to_string()).collect(),
        None => vec![s.to_string()],
    }
}

/// Parse a `<col> -> <ref_table>.<ref_col> [on_delete=<a>] [on_update=<a>]` FK line (the
/// `fk ` prefix already stripped). A composite FK carries `(c1, c2)` tuples on both sides.
fn parse_fk(rest: &str, line: usize) -> Result<ForeignKeySnap, ParseError> {
    let malformed = || ParseError {
        line,
        message: format!("malformed fk: {rest}"),
    };
    let (col, tail) = rest.split_once("->").ok_or_else(malformed)?;
    let tail = tail.trim();
    // Split the `<ref_table>.<ref_cols>` reference from any trailing `on_*` attributes; the
    // reference's `(p1, p2)` tuple carries spaces, so we can't just `split_whitespace`.
    let attr_at = tail
        .find(" on_delete=")
        .or_else(|| tail.find(" on_update="));
    let (reference, attrs) = match attr_at {
        Some(i) => (&tail[..i], &tail[i..]),
        None => (tail, ""),
    };
    let (ref_table, ref_cols) = reference.trim().split_once('.').ok_or_else(malformed)?;
    // A `schema::table` reference carries a cross-schema target; a bare name is default.
    let (ref_schema, ref_table) = match ref_table.trim().split_once("::") {
        Some((sc, tbl)) => (Some(sc.trim().to_string()), tbl.trim().to_string()),
        None => (None, ref_table.trim().to_string()),
    };
    let mut on_delete = None;
    let mut on_update = None;
    for tok in attrs.split_whitespace() {
        if let Some(a) = tok.strip_prefix("on_delete=") {
            on_delete = Some(a.to_string());
        } else if let Some(a) = tok.strip_prefix("on_update=") {
            on_update = Some(a.to_string());
        } else {
            return Err(malformed());
        }
    }
    Ok(ForeignKeySnap {
        columns: parse_col_list(col),
        ref_table,
        ref_schema,
        ref_columns: parse_col_list(ref_cols),
        on_delete,
        on_update,
    })
}

/// Byte length of the balanced `raw(…)` token at the head of `s`, counting parens
/// outside string literals. `None` when it never closes (a corrupt snapshot).
fn balanced_end(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        match c {
            _ if esc => esc = false,
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Pull the space-carrying column attributes — a quoted `default="…"`, an opaque `raw(…)`
/// type, and a `generated=(<expr>)` body — out of the raw column text so the rest
/// tokenizes on whitespace cleanly. Returns `(default, raw_type, generated, remainder)`.
fn extract_spaced_attrs(rest: &str) -> (Option<String>, Option<String>, Option<String>, String) {
    // A quoted `default="…"` is the one attribute that may hold spaces; pull it out of
    // the raw text first.
    let mut default = None;
    let mut remainder = rest.to_string();
    if let Some(idx) = rest.find("default=\"") {
        let after = &rest[idx + "default=".len()..]; // starts at the opening quote
        if let Some(close) = after[1..].find('"') {
            let end = close + 2; // include both quotes
            default = Some(after[..end].to_string());
            remainder = format!("{}{}", &rest[..idx], &after[end..]);
        }
    }

    // An opaque `raw(…)` type may hold spaces and commas; take it as one balanced token.
    let mut raw_ty = None;
    if let Some(idx) = remainder.find("raw(") {
        if let Some(end) = balanced_end(&remainder[idx..]) {
            raw_ty = Some(remainder[idx..idx + end].to_string());
            remainder = format!("{}{}", &remainder[..idx], &remainder[idx + end..]);
        }
    }

    // A generated column's `generated=(<expr>)` body holds spaces; pull it out as one
    // balanced token (from the opening paren).
    let mut generated = None;
    if let Some(pos) = remainder.find("generated=(") {
        let open = pos + "generated=".len(); // index of the `(`
        if let Some(end) = balanced_end(&remainder[open..]) {
            let inner = &remainder[open + 1..open + end - 1]; // strip the parens
            generated = Some(inner.to_string());
            remainder = format!("{}{}", &remainder[..pos], &remainder[open + end..]);
        }
    }

    (default, raw_ty, generated, remainder)
}

fn parse_column(rest: &str, line: usize) -> Result<ColumnSnap, ParseError> {
    let (mut default, raw_ty, generated, remainder) = extract_spaced_attrs(rest);

    let mut toks = remainder.split_whitespace();
    let name = toks.next().ok_or_else(|| ParseError {
        line,
        message: "column has no name".to_string(),
    })?;
    let ty = match &raw_ty {
        Some(t) => t.as_str(),
        None => toks.next().ok_or_else(|| ParseError {
            line,
            message: "column has no type".to_string(),
        })?,
    };
    let nullability = toks.next().ok_or_else(|| ParseError {
        line,
        message: "column has no nullability".to_string(),
    })?;
    let nullable = match nullability {
        "null" => true,
        "not_null" => false,
        other => {
            return Err(ParseError {
                line,
                message: format!("expected null|not_null, got {other}"),
            })
        }
    };

    let mut unique = false;
    let mut fk = None;
    for tok in toks {
        if let Some(d) = tok.strip_prefix("default=") {
            default = Some(d.to_string());
        } else if tok == "unique" {
            unique = true;
        } else if let Some(t) = tok.strip_prefix("fk=") {
            fk = Some(t.to_string());
        } else {
            return Err(ParseError {
                line,
                message: format!("unrecognized column attribute: {tok}"),
            });
        }
    }

    Ok(ColumnSnap {
        name: name.to_string(),
        ty: ty.to_string(),
        nullable,
        default,
        unique,
        fk,
        generated,
    })
}

fn parse_index(rest: &str, line: usize) -> Result<IndexSnap, ParseError> {
    // An opaque index is `<name> raw(…)`: everything after the name is the literal body.
    if let Some((name, body)) = rest.trim().split_once(char::is_whitespace) {
        let body = body.trim();
        if body.starts_with("raw(") {
            return Ok(IndexSnap {
                name: name.trim().to_string(),
                columns: Vec::new(),
                unique: false,
                method: None,
                raw: Some(body.to_string()),
            });
        }
    }
    let (name, after) = rest.split_once('(').ok_or_else(|| ParseError {
        line,
        message: format!("index missing column list: {rest}"),
    })?;
    let close = after.find(')').ok_or_else(|| ParseError {
        line,
        message: format!("index column list not closed: {rest}"),
    })?;
    let cols_inner = &after[..close];
    let columns: Vec<String> = if cols_inner.trim().is_empty() {
        Vec::new()
    } else {
        cols_inner
            .split(',')
            .map(|s| s.trim().to_string())
            .collect()
    };
    let flags = after[close + 1..].trim();
    let mut toks = flags.split_whitespace();
    let mut unique = false;
    let mut method = None;
    while let Some(tok) = toks.next() {
        match tok {
            "unique" => unique = true,
            "using" => method = toks.next().map(str::to_string),
            // Unknown flags are ignored, not rejected — a snapshot written by a newer or
            // older engine (e.g. an older `inferred` marker) must still parse so an
            // existing ledger keeps applying.
            _ => {}
        }
    }

    Ok(IndexSnap {
        name: name.trim().to_string(),
        columns,
        unique,
        method,
        raw: None,
    })
}
