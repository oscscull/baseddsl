use super::*;

/// Build one `DocumentSymbol` from a declaration's name + extent span.
#[allow(deprecated)] // `deprecated` is a required struct field, set to None.
fn symbol(
    name: &Ident,
    kind: SymbolKind,
    extent: Span,
    idx: &LineIndex,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    DocumentSymbol {
        name: name.node.clone(),
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range: span_range(extent, idx),
        selection_range: span_range(name.span, idx),
        children,
    }
}

/// One `workspace/symbol` result: a flat, file-located symbol (no nesting — that is
/// what `container_name` is for).
#[allow(deprecated)] // `deprecated` is a required struct field, set to None.
fn sym_info(
    name: &str,
    kind: SymbolKind,
    location: Location,
    container: Option<&str>,
) -> SymbolInformation {
    SymbolInformation {
        name: name.to_owned(),
        kind,
        tags: None,
        deprecated: None,
        location,
        container_name: container.map(str::to_owned),
    }
}

/// Case-insensitive fuzzy subsequence match, the ⌘T convention: every char of
/// `query` must appear in `name` in order (not necessarily contiguously). An empty
/// query matches everything. The client re-ranks; this is the coarse server filter.
fn fuzzy_match(query: &str, name: &str) -> bool {
    let mut chars = name.chars().flat_map(char::to_lowercase);
    for q in query.chars().flat_map(char::to_lowercase) {
        if !chars.any(|c| c == q) {
            return false;
        }
    }
    true
}

impl Snapshot {
    /// Document symbols for file `fid` — the outline the editor shows (breadcrumbs
    /// / ⇧⌘O). Models nest their fields as `Field` children; queries, mutations,
    /// shapes, and filters are flat top-level symbols. Each symbol's `range` is its
    /// declaration extent and its `selection_range` the name (both required to nest,
    /// LSP contains the latter in the former). Only decls declared in `fid` appear.
    pub fn document_symbols(&self, fid: usize) -> Vec<DocumentSymbol> {
        let idx = &self.lines[fid];
        let here = |span: Span| span.file.0 as usize == fid;
        let mut out = Vec::new();
        for d in &self.decls {
            match d {
                // Model → Struct; its fields (not indexes / soft-overrides) → Field children.
                Decl::Model(m) if here(m.span) => {
                    let children = m
                        .members
                        .iter()
                        .filter_map(|mem| match mem {
                            Member::Field(f) => {
                                Some(symbol(&f.name, SymbolKind::FIELD, f.span, idx, None))
                            }
                            Member::Generated(g) => {
                                Some(symbol(&g.name, SymbolKind::FIELD, g.span, idx, None))
                            }
                            _ => None,
                        })
                        .collect();
                    out.push(symbol(
                        &m.name,
                        SymbolKind::STRUCT,
                        m.span,
                        idx,
                        Some(children),
                    ));
                }
                // Enum → Enum; its variants → EnumMember children.
                Decl::Enum(e) if here(e.span) => {
                    let children = e
                        .variants
                        .iter()
                        .map(|v| symbol(&v.name, SymbolKind::ENUM_MEMBER, v.name.span, idx, None))
                        .collect();
                    out.push(symbol(
                        &e.name,
                        SymbolKind::ENUM,
                        e.span,
                        idx,
                        Some(children),
                    ));
                }
                Decl::Shape(s) if here(s.span) => {
                    out.push(symbol(&s.name, SymbolKind::INTERFACE, s.span, idx, None));
                }
                Decl::Query(q) if here(q.span) => {
                    out.push(symbol(&q.name, SymbolKind::FUNCTION, q.span, idx, None));
                }
                Decl::Mutation(m) if here(m.span) => {
                    out.push(symbol(&m.name, SymbolKind::METHOD, m.span, idx, None));
                }
                Decl::Filter(f) if here(f.span) => {
                    out.push(symbol(&f.name, SymbolKind::FUNCTION, f.span, idx, None));
                }
                _ => {}
            }
        }
        out
    }

    /// Workspace symbols (`workspace/symbol`, ⌘T): every named declaration across the
    /// whole project — models (with their fields), shapes, scopes, queries, mutations,
    /// filters — filtered by a case-insensitive fuzzy subsequence match on `query`
    /// (empty query = everything). Unlike [`Snapshot::document_symbols`] this spans
    /// every file in the snapshot; each symbol carries its own file `Location` so the
    /// editor jumps straight to the declaration.
    pub fn workspace_symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let mut out = Vec::new();
        let mut push = |name: &Ident, kind: SymbolKind, container: Option<&str>| {
            if !fuzzy_match(query, &name.node) {
                return;
            }
            let fid = name.span.file.0 as usize;
            let Some((path, _)) = self.sources.get(fid) else {
                return;
            };
            let Ok(uri) = Url::from_file_path(path) else {
                return;
            };
            let location = Location::new(uri, span_range(name.span, &self.lines[fid]));
            out.push(sym_info(&name.node, kind, location, container));
        };
        for d in &self.decls {
            match d {
                Decl::Model(m) => {
                    push(&m.name, SymbolKind::STRUCT, None);
                    for mem in &m.members {
                        match mem {
                            Member::Field(f) => {
                                push(&f.name, SymbolKind::FIELD, Some(&m.name.node));
                            }
                            Member::Generated(g) => {
                                push(&g.name, SymbolKind::FIELD, Some(&m.name.node));
                            }
                            _ => {}
                        }
                    }
                }
                Decl::Shape(s) => push(&s.name, SymbolKind::INTERFACE, None),
                Decl::Scope(s) => push(&s.name, SymbolKind::NAMESPACE, None),
                Decl::Enum(e) => {
                    push(&e.name, SymbolKind::ENUM, None);
                    for v in &e.variants {
                        push(&v.name, SymbolKind::ENUM_MEMBER, Some(&e.name.node));
                    }
                }
                Decl::Query(q) => push(&q.name, SymbolKind::FUNCTION, None),
                Decl::Mutation(m) => push(&m.name, SymbolKind::METHOD, None),
                Decl::Filter(f) => push(&f.name, SymbolKind::FUNCTION, None),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    /// Document symbols for a file expose its models (with fields nested), shapes,
    /// queries, and mutations with the right kinds — asserted over the real commerce
    /// schema so nesting + kind mapping are proven end to end.
    #[test]
    fn document_symbols_over_commerce_order_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        // order/model.bsl: the `Order` model (Struct) with its fields nested, plus
        // the `OrderCard` shape (Interface) — both flat top-level symbols.
        let model_fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let syms = snap.document_symbols(model_fid);
        let order = syms
            .iter()
            .find(|s| s.name == "Order")
            .expect("Order symbol");
        assert_eq!(order.kind, SymbolKind::STRUCT);
        let fields = order.children.as_ref().expect("Order has field children");
        assert!(fields.iter().all(|f| f.kind == SymbolKind::FIELD));
        for want in ["org", "placed_by", "status", "total", "items"] {
            assert!(fields.iter().any(|f| f.name == want), "field {want}");
        }
        // The name selection range sits inside the declaration extent.
        assert!(order.selection_range.start >= order.range.start);
        assert!(order.selection_range.end <= order.range.end);
        let card = syms
            .iter()
            .find(|s| s.name == "OrderCard")
            .expect("OrderCard shape");
        assert_eq!(card.kind, SymbolKind::INTERFACE);

        // order/queries.bsl: queries → Function, the mutation → Method.
        let q_fid = snap.file_id_of(&root.join("order/queries.bsl")).unwrap();
        let qsyms = snap.document_symbols(q_fid);
        let q = qsyms
            .iter()
            .find(|s| s.name == "my_org_orders")
            .expect("query symbol");
        assert_eq!(q.kind, SymbolKind::FUNCTION);
        let m = qsyms
            .iter()
            .find(|s| s.name == "place_order")
            .expect("mutation symbol");
        assert_eq!(m.kind, SymbolKind::METHOD);
        // Symbols are file-scoped: no cross-file leakage into the query file.
        assert!(qsyms.iter().all(|s| s.name != "Order"));
    }

    /// Workspace symbols span every file in the project (unlike document symbols),
    /// map each declaration kind, nest fields under their model via `container_name`,
    /// and filter by a fuzzy subsequence query. Asserted over the commerce schema.
    #[test]
    fn workspace_symbols_span_project_and_filter() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        // Empty query = everything. Symbols come from many files, each carrying its
        // own file `Location` (workspace-wide, not one file).
        let all = snap.workspace_symbols("");
        let files: HashSet<_> = all.iter().map(|s| s.location.uri.to_string()).collect();
        assert!(files.len() > 1, "symbols should span multiple files");

        // The `Order` model (Struct) and its `status` field (Field, contained in Order).
        let order = all
            .iter()
            .find(|s| s.name == "Order")
            .expect("Order symbol");
        assert_eq!(order.kind, SymbolKind::STRUCT);
        let status = all
            .iter()
            .find(|s| s.name == "status" && s.container_name.as_deref() == Some("Order"))
            .expect("Order.status field");
        assert_eq!(status.kind, SymbolKind::FIELD);

        // Kind mapping across decl kinds: shape → Interface, query → Function,
        // mutation → Method.
        assert_eq!(
            all.iter().find(|s| s.name == "OrderCard").unwrap().kind,
            SymbolKind::INTERFACE
        );
        assert_eq!(
            all.iter().find(|s| s.name == "place_order").unwrap().kind,
            SymbolKind::METHOD
        );
        assert_eq!(
            all.iter().find(|s| s.name == "my_org_orders").unwrap().kind,
            SymbolKind::FUNCTION
        );

        // Fuzzy subsequence filter, case-insensitive: "oc" matches OrderCard (O..C),
        // and the query narrows the set. A non-subsequence query drops it.
        let oc = snap.workspace_symbols("oc");
        assert!(oc.iter().any(|s| s.name == "OrderCard"));
        assert!(oc.len() < all.len());
        assert!(snap
            .workspace_symbols("zzq")
            .iter()
            .all(|s| s.name != "OrderCard"));
    }

    /// `fuzzy_match` is an ordered, case-insensitive subsequence test; empty matches all.
    #[test]
    fn fuzzy_match_is_ordered_subsequence() {
        assert!(fuzzy_match("", "anything"));
        assert!(fuzzy_match("oc", "OrderCard"));
        assert!(fuzzy_match("order", "Order"));
        assert!(fuzzy_match("PO", "place_order"));
        assert!(!fuzzy_match("co", "OrderCard")); // out of order
        assert!(!fuzzy_match("xyz", "Order"));
    }
}
