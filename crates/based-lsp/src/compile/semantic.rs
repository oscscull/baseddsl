use super::*;


/// Append one highlight span `[start, end)` (byte offsets in `src`) to `out` as
/// `(line, utf16-char, utf16-len, token-type)`, splitting at newlines so no emitted
/// token crosses a line (an LSP semantic token is single-line).
fn push_semantic_token(
    src: &str,
    idx: &LineIndex,
    start: usize,
    end: usize,
    token_type: u32,
    out: &mut Vec<(u32, u32, u32, u32)>,
) {
    let mut lo = start;
    while lo < end {
        let pos = idx.position(lo);
        let line_stop = src[lo..end].find('\n').map_or(end, |n| lo + n);
        let seg = &src[lo..line_stop];
        let len16: u32 = seg.chars().map(|c| c.len_utf16() as u32).sum();
        if len16 > 0 {
            out.push((pos.line, pos.character, len16, token_type));
        }
        lo = if line_stop < end { line_stop + 1 } else { end };
    }
}


/// Every `raw`…`` block declared anywhere in `d` — value, shape, index-adjacent, query,
/// mutation, and filter positions — for per-dialect SQL highlighting.
fn collect_raw_in_decl<'a>(d: &'a Decl, out: &mut Vec<&'a RawSql>) {
    match d {
        Decl::Model(m) => {
            for mem in &m.members {
                if let Member::SoftOverride(so) = mem {
                    out.push(&so.raw);
                }
            }
        }
        Decl::Shape(s) => s.body.iter().for_each(|f| shape_field_raw(f, out)),
        Decl::Query(q) => match &q.body {
            QueryBody::Inline(cs) => cs.iter().for_each(|c| clause_raw(c, out)),
            QueryBody::Block(st) => st.clauses.iter().for_each(|c| clause_raw(c, out)),
            QueryBody::Raw(r) => out.push(r),
            QueryBody::Bare => {}
        },
        Decl::Mutation(m) => write_raw(&m.body, out),
        Decl::Filter(f) => pred_raw(&f.pred, out),
        _ => {}
    }
}


fn shape_field_raw<'a>(f: &'a ShapeField, out: &mut Vec<&'a RawSql>) {
    match f {
        ShapeField::Rename { value, .. } => shape_value_raw(value, out),
        ShapeField::Nest { body, .. } | ShapeField::Flatten { body, .. } => {
            for sf in body {
                shape_field_raw(sf, out);
            }
        }
        ShapeField::Bare(_) | ShapeField::NestRef { .. } | ShapeField::Spread { .. } => {}
    }
}


fn shape_value_raw<'a>(v: &'a ShapeValue, out: &mut Vec<&'a RawSql>) {
    match v {
        ShapeValue::Raw(r) => out.push(r),
        ShapeValue::Computed(e) => shape_expr_raw(e, out),
        ShapeValue::Path(_) | ShapeValue::Agg(_) => {}
    }
}


fn shape_expr_raw<'a>(e: &'a ShapeExpr, out: &mut Vec<&'a RawSql>) {
    match e {
        ShapeExpr::Arith { lhs, rhs, .. } | ShapeExpr::Concat { lhs, rhs, .. } => {
            shape_expr_raw(lhs, out);
            shape_expr_raw(rhs, out);
        }
        ShapeExpr::Case { arms, else_, .. } => {
            for a in arms {
                pred_raw(&a.when, out);
                shape_expr_raw(&a.then, out);
            }
            shape_expr_raw(else_, out);
        }
        ShapeExpr::Value(_) => {}
    }
}


fn clause_raw<'a>(c: &'a Clause, out: &mut Vec<&'a RawSql>) {
    if let Clause::Where(p) | Clause::Having(p) = c {
        pred_raw(p, out);
    }
}


fn pred_raw<'a>(p: &'a Predicate, out: &mut Vec<&'a RawSql>) {
    match p {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            pred_raw(a, out);
            pred_raw(b, out);
        }
        Predicate::Not(inner) => pred_raw(inner, out),
        Predicate::Raw(r) => out.push(r),
        Predicate::Cmp { .. }
        | Predicate::InList { .. }
        | Predicate::FilterCall { .. }
        | Predicate::Bare(_) => {}
    }
}


fn write_raw<'a>(body: &'a [WriteStmt], out: &mut Vec<&'a RawSql>) {
    for w in body {
        match w {
            WriteStmt::Update { where_, .. } | WriteStmt::Restore { where_, .. } => {
                pred_raw(where_, out);
            }
            WriteStmt::Delete { where_, .. } | WriteStmt::HardDelete { where_, .. } => {
                if let Some(p) = where_ {
                    pred_raw(p, out);
                }
            }
            WriteStmt::Tx(inner) => write_raw(inner, out),
            WriteStmt::Raw(r) => out.push(r),
            WriteStmt::Create { .. } => {}
        }
    }
}


impl Snapshot {
    /// LSP semantic tokens highlighting the SQL inside every `raw`…`` block in file
    /// `fid`, tokenized against the project's compile-target dialect (a loose file uses
    /// the documented default). Returns the protocol's relative-encoded token stream.
    pub fn semantic_tokens(&self, fid: usize) -> Vec<SemanticToken> {
        let Some((_, src)) = self.sources.get(fid) else {
            return Vec::new();
        };
        let idx = &self.lines[fid];
        let dialect = self.dialect.unwrap_or(based_codegen::Dialect::MariaDb);

        // Every raw block declared in this file, in source order.
        let mut raws: Vec<&RawSql> = Vec::new();
        for d in &self.decls {
            collect_raw_in_decl(d, &mut raws);
        }
        raws.retain(|r| r.span.file.0 as usize == fid);
        raws.sort_by_key(|r| r.span.start);

        // Absolute (line, utf16-char, utf16-len, token-type) tokens, split so none
        // crosses a line (LSP tokens are single-line).
        let mut abs: Vec<(u32, u32, u32, u32)> = Vec::new();
        for r in raws {
            let (span_lo, span_hi) = (r.span.start as usize, r.span.end as usize);
            let Some(block) = src.get(span_lo..span_hi) else {
                continue;
            };
            // The interior sits between the opening backtick (after the `raw` keyword)
            // and the closing backtick at the span's end.
            let Some(open_rel) = block.find('`') else {
                continue;
            };
            let interior_start = span_lo + open_rel + 1;
            let interior_end = span_hi.saturating_sub(1);
            let Some(interior) = src.get(interior_start..interior_end) else {
                continue;
            };
            for t in sqltok::tokenize(interior, dialect) {
                let lo = interior_start + t.start;
                push_semantic_token(src, idx, lo, lo + t.len, t.kind.type_index(), &mut abs);
            }
        }

        // Relative-encode: each token carries its line/char delta from the previous.
        let mut out = Vec::with_capacity(abs.len());
        let (mut prev_line, mut prev_char) = (0u32, 0u32);
        for (line, ch, len, ty) in abs {
            out.push(SemanticToken {
                delta_line: line - prev_line,
                delta_start: if line == prev_line {
                    ch - prev_char
                } else {
                    ch
                },
                length: len,
                token_type: ty,
                token_modifiers_bitset: 0,
            });
            prev_line = line;
            prev_char = ch;
        }
        out
    }

}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    /// Raw blocks in every position — shape value, `where` predicate, whole-query body,
    /// and a soft-delete override — are each tokenized, with offsets landing on the SQL
    /// interior only (never the `raw` marker or the backticks).
    #[test]
    fn semantic_tokens_cover_every_raw_position() {
        let ws = TempWorkspace::new("sem_positions");
        ws.write("based.toml", "dialect = \"mariadb\"");
        ws.write(
            "schema.bsl",
            "@soft_delete(deleted_at)\n\
             User {\n\
               name: text\n\
               first: text\n\
               last: text\n\
               total: int\n\
               deleted_at: timestamp?\n\
               restore: raw`update {table} set deleted_at = null where {id}`\n\
             }\n\
             shape UserRow from User { full = raw`concat(first, ' ', last)` }\n\
             query heavy(min: int) -> UserRow[] {\n\
               list User where (raw`total >= ${min}`);\n\
             }\n\
             query allraw(min: int) -> UserRow[] {\n\
               raw`select name from user where total >= ${min}`;\n\
             }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let toks = decode_semantic(&snap, fid);

        // The soft-override raw: keywords + engine interpolations.
        assert!(toks.contains(&("update".into(), "keyword".into())));
        assert!(toks.contains(&("{table}".into(), "macro".into())));
        assert!(toks.contains(&("{id}".into(), "macro".into())));

        // The shape value raw: `concat` is a bare word (no token), the `' '` is a string.
        assert!(toks.iter().any(|(t, k)| t == "' '" && k == "string"));
        assert!(!toks.iter().any(|(t, _)| t == "concat"));

        // The `where` predicate raw + the whole-query raw both bind `${min}` as a param.
        let params: Vec<_> = toks.iter().filter(|(t, _)| t == "${min}").collect();
        assert_eq!(params.len(), 2, "one param per raw block: {toks:?}");
        assert!(params.iter().all(|(_, k)| k == "parameter"));

        // `select`/`from`/`where` in the whole-query raw are keywords.
        for kw in ["select", "from", "where"] {
            assert!(
                toks.contains(&(kw.into(), "keyword".into())),
                "{kw} missing in {toks:?}"
            );
        }
        // The `raw` marker and the delimiting backticks are never emitted — SQL only.
        assert!(toks.iter().all(|(t, _)| t != "raw" && !t.contains('`')));
    }


    /// The highlighting dialect follows the manifest: a Postgres project reads `"col"`
    /// as an identifier, a MariaDB project reads it as a string.
    #[test]
    fn semantic_tokens_follow_manifest_dialect() {
        fn tokens_for(tag: &str, dialect: &str) -> Vec<(String, String)> {
            let ws = TempWorkspace::new(tag);
            ws.write("based.toml", &format!("dialect = \"{dialect}\""));
            ws.write(
                "schema.bsl",
                "User { name: text, total: int }\n\
                 shape R from User { name }\n\
                 query q() -> R[] { raw`select \"total\" from user`; }\n",
            );
            let snap = compile_manifest(&ws.root, &HashMap::new());
            let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
            decode_semantic(&snap, fid)
        }

        let pg = tokens_for("sem_pg", "postgres");
        assert!(
            pg.contains(&("\"total\"".into(), "variable".into())),
            "postgres reads `\"total\"` as an identifier: {pg:?}"
        );
        let maria = tokens_for("sem_maria", "mariadb");
        assert!(
            maria.contains(&("\"total\"".into(), "string".into())),
            "mariadb reads `\"total\"` as a string: {maria:?}"
        );
    }

}
