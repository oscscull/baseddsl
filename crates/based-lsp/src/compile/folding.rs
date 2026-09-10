use super::*;


/// A top-level declaration's extent span (name/decorators through the closing
/// delimiter) — the anchor folding and selection ranges expand to.
fn decl_span(d: &Decl) -> Span {
    match d {
        Decl::Model(m) => m.span,
        Decl::Shape(s) => s.span,
        Decl::Scope(s) => s.span,
        Decl::Enum(e) => e.span,
        Decl::Query(q) => q.span,
        Decl::Mutation(m) => m.span,
        Decl::Filter(f) => f.span,
    }
}


impl Snapshot {
    pub fn folding_ranges(&self, fid: usize) -> Vec<FoldingRange> {
        let idx = &self.lines[fid];
        let src = &self.sources[fid].1;
        let mut out = Vec::new();
        for d in &self.decls {
            let span = decl_span(d);
            if span.file.0 as usize != fid {
                continue;
            }
            let (lo, hi) = (span.start as usize, (span.end as usize).min(src.len()));
            // Fold from the block's opening brace when there is one, else the decl's
            // own start; the close is the span's last byte (the `}` / `)` / `;`).
            let open = src[lo..hi].find('{').map_or(lo, |i| lo + i);
            let start_line = idx.position(open).line;
            let end_line = idx.position(hi.saturating_sub(1)).line;
            if end_line > start_line {
                out.push(FoldingRange {
                    start_line,
                    end_line,
                    kind: Some(FoldingRangeKind::Region),
                    ..Default::default()
                });
            }
        }
        out
    }


    /// The selection-range hierarchy at `offset`: the nested ranges an editor cycles
    /// through on expand/shrink-selection, innermost first. Levels are the identifier
    /// token under the cursor, then each enclosing AST span that covers it (a model
    /// field's name → type → whole field, its declaration, and finally the whole
    /// file), each linked to its parent. `None` when the offset sits in no declaration
    /// and on no word (the caller falls back to a bare cursor range).
    pub fn selection_range(&self, fid: usize, offset: u32) -> Option<SelectionRange> {
        let src = &self.sources[fid].1;
        let contains =
            |sp: Span| sp.file.0 as usize == fid && sp.start <= offset && offset < sp.end;

        // Candidate ranges covering the offset, from the AST spans plus the word token
        // and the whole file. Order is imposed below by width, so collection order is
        // free.
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        if let Some((s, e)) = word_extent(src, offset) {
            ranges.push((s as u32, e as u32));
        }
        for d in &self.decls {
            let dspan = decl_span(d);
            if !contains(dspan) {
                continue;
            }
            if let Decl::Model(m) = d {
                for mem in &m.members {
                    if let Member::Field(f) = mem {
                        if contains(f.span) {
                            for sp in [f.name.span, f.ty.span, f.span] {
                                if contains(sp) {
                                    ranges.push((sp.start, sp.end));
                                }
                            }
                        }
                    }
                }
            }
            ranges.push((dspan.start, dspan.end));
        }
        ranges.push((0, src.len() as u32));

        // Keep a strictly-nesting chain, narrowest first: sort by width, drop
        // duplicates and any range that doesn't contain the one kept before it.
        ranges.sort_by_key(|(s, e)| e - s);
        ranges.dedup();
        let mut chain: Vec<(u32, u32)> = Vec::new();
        for (s, e) in ranges {
            match chain.last() {
                Some(&(ps, pe)) if s <= ps && e >= pe && (s, e) != (ps, pe) => chain.push((s, e)),
                None => chain.push((s, e)),
                _ => {}
            }
        }

        // Build parent links from the widest inward; the returned root is innermost.
        let idx = &self.lines[fid];
        let mut node: Option<Box<SelectionRange>> = None;
        for (s, e) in chain.into_iter().rev() {
            let range = Range::new(idx.position(s as usize), idx.position(e as usize));
            node = Some(Box::new(SelectionRange {
                range,
                parent: node,
            }));
        }
        node.map(|b| *b)
    }

}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    /// Folding ranges cover each multi-line declaration body: the `Order` model folds
    /// from its `{` header line to its closing brace, the `OrderCard` shape likewise,
    /// while the single-line `scope Tenant (…)` yields no fold.
    #[test]
    fn folding_ranges_over_commerce_order_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[fid].1;
        let idx = &snap.lines[fid];
        let ranges = snap.folding_ranges(fid);

        // The `Order { … }` model: fold from the `{` header line to its `}` line.
        let open = src.find("Order {").unwrap() + "Order ".len();
        let close = src[open..].find('}').map(|i| open + i).unwrap();
        let open_line = idx.position(open).line;
        let close_line = idx.position(close).line;
        assert!(close_line > open_line);
        assert!(
            ranges.iter().any(|r| r.start_line == open_line
                && r.end_line == close_line
                && r.kind == Some(FoldingRangeKind::Region)),
            "Order body should fold {open_line}..{close_line}: {ranges:?}"
        );

        // The single-line `scope Tenant (…)` decl has nothing to fold.
        let scope_line = idx.position(src.find("scope Tenant").unwrap()).line;
        assert!(
            !ranges.iter().any(|r| r.start_line == scope_line),
            "a single-line decl must not fold: {ranges:?}"
        );

        // Both the model and the shape fold (two multi-line decls in the file).
        assert!(ranges.len() >= 2, "{ranges:?}");
    }


    /// A selection range expands outward through the AST: the `total` token → its
    /// field declaration → the enclosing `Order` model → the whole file, each range
    /// strictly containing the one it parents.
    #[test]
    fn selection_range_expands_token_to_field_to_model_to_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[fid].1;
        let idx = &snap.lines[fid];

        // Cursor inside the `total: int` field name.
        let off = (src.find("total:").unwrap() + 1) as u32;
        let sr = snap
            .selection_range(fid, off)
            .expect("a selection hierarchy");

        // Walk the chain innermost → outermost, capturing each level's source text.
        let mut texts = Vec::new();
        let mut cur = Some(&sr);
        while let Some(node) = cur {
            let s = idx.offset(node.range.start);
            let e = idx.offset(node.range.end);
            texts.push(src[s..e].to_string());
            cur = node.parent.as_deref();
        }

        // Innermost is the token; a field level carries `total` + its `decimal` type; a
        // model level spans the whole `Order` body; the outermost is the whole file.
        assert_eq!(texts.first().unwrap(), "total");
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with("total") && t.contains("decimal")),
            "a field-level range: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Order {") && t.contains("items")),
            "a model-level range: {texts:?}"
        );
        assert_eq!(
            texts.last().unwrap().len(),
            src.len(),
            "outermost is the whole file"
        );

        // Strictly nesting: each level is wider than the one it parents.
        for w in texts.windows(2) {
            assert!(
                w[0].len() < w[1].len(),
                "ranges must strictly nest: {texts:?}"
            );
        }
    }

}
