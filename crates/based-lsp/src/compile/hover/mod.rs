use super::*;
mod text;
use text::*;

impl Snapshot {
    /// Rich "what" for the symbol under the cursor — a field's `name: Type`, or a
    /// model/shape/scope/callable signature — for hover (rust-analyzer's baseline).
    /// `None` when the cursor is not on a resolvable symbol.
    pub fn hover_at(&self, fid: usize, offset: u32) -> Option<String> {
        let under = |id: &&Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        // An enum variant (use or declaration) → its enum and value.
        if let Some(decl) = self.variant_ref_at(fid, offset) {
            if let Some(h) = self.variant_hover(decl) {
                return Some(h);
            }
        }
        // A signature param binding (or an unbound bare/inline param) → the
        // predicate it generates, with the bound field's signature when the cursor
        // is on the column/edge ident itself.
        if let Some(h) = self.binding_hover(fid, offset) {
            return Some(h);
        }
        // A `tx` step binding (`as name` decl or a `$name.field` reference) → what it is:
        // the model of the row it binds. Tried before field references so the `$name`
        // head is never misread.
        if let Some(h) = self.step_binding_hover(fid, offset) {
            return Some(h);
        }
        // A field reference (path segment) → the field's signature.
        if let Some(f) = self.field_ref_at(fid, offset) {
            return Some(field_hover(f));
        }
        // A model/shape type reference → the referenced decl's signature.
        if let Some(t) = collect_type_refs(&self.decls).into_iter().find(under) {
            if let Some(h) = self.decl_hover_by_name(&t.node) {
                return Some(h);
            }
        }
        // A `@scope`/`scoped` reference → the scope contract's shape.
        if let Some(t) = collect_scope_refs(&self.decls).into_iter().find(under) {
            if let Some(h) = self.scope_hover(&t.node) {
                return Some(h);
            }
        }
        // Otherwise the cursor may sit on a declaration's own name.
        self.decl_site_hover(fid, offset)
    }

    /// Hover for a `tx` step binding: names the model of the row it binds, so a reader
    /// knows what `$name.field` reaches. `None` off any binding decl / `$name` head.
    fn step_binding_hover(&self, fid: usize, offset: u32) -> Option<String> {
        let target = self.binding_ref_at(fid, offset)?;
        let name = self.span_text(target)?.to_string();
        for d in &self.decls {
            if let Decl::Mutation(m) = d {
                if let Some(model) = binding_model(&m.body, target) {
                    return Some(format!(
                        "```based\nstep binding {name}: {model}\n```\nthe row this `tx` step creates — reference it as `${name}.field`",
                    ));
                }
            }
        }
        None
    }

    /// The predicate a signature param binding generates, when the cursor sits on
    /// the binding — the column/edge ident or the operator/arrow between it and the
    /// param head. On the ident itself the bound field's signature leads. An
    /// *unbound* param of a bare/inline query carries the derived same-name
    /// equality (`status: text` → `status = $status`), anchored at the param name.
    fn binding_hover(&self, fid: usize, offset: u32) -> Option<String> {
        let within = |sp: Span| sp.file.0 as usize == fid && sp.start <= offset && offset < sp.end;
        for d in &self.decls {
            let Decl::Query(q) = d else { continue };
            let root = match &q.body {
                QueryBody::Block(stmt) => Some(stmt.model.node.as_str()),
                _ => self.query_root(q),
            };
            let Some(root) = root else { continue };
            for p in &q.params {
                // The binding region runs from the end of the param head (its type
                // annotation, or the bare name) through the bound ident, so the
                // `->` / operator token between them is hoverable too.
                let head_end = p.ty.as_ref().map_or(p.name.span.end, |t| t.span.end);
                match &p.binding {
                    Some(ParamBinding::Edge(edge)) => {
                        if edge.span.file.0 as usize == fid
                            && head_end <= offset
                            && offset < edge.span.end
                        {
                            let line = format!(
                                "binds `{} = ${}` — via the `{}` relation edge",
                                edge.node, p.name.node, edge.node
                            );
                            return Some(self.with_field_sig(root, edge, within, line));
                        }
                    }
                    Some(ParamBinding::ColOp { op, col }) => {
                        if col.span.file.0 as usize == fid
                            && head_end <= offset
                            && offset < col.span.end
                        {
                            let line = format!(
                                "binds `{} {} ${}` — {}",
                                col.node,
                                op_str(*op),
                                p.name.node,
                                op_gloss(*op)
                            );
                            return Some(self.with_field_sig(root, col, within, line));
                        }
                    }
                    // Bare/inline queries bind an unbound param to its same-named
                    // column; block/raw queries reference params via `$`, so no fact.
                    None => {
                        if within(p.name.span)
                            && matches!(q.body, QueryBody::Bare | QueryBody::Inline(_))
                        {
                            let slice = std::slice::from_ref(&p.name);
                            if self.walk_path(root, slice).is_some() {
                                let line = format!(
                                    "binds `{n} = ${n}` — an unbound param binds its same-named column",
                                    n = p.name.node
                                );
                                return Some(self.with_field_sig(root, &p.name, within, line));
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Prepend `ident`'s field signature to `line` when the cursor is on the ident
    /// and it resolves against `root`; the bare binding line otherwise.
    fn with_field_sig(
        &self,
        root: &str,
        ident: &Ident,
        within: impl Fn(Span) -> bool,
        line: String,
    ) -> String {
        if within(ident.span) {
            if let Some(f) = self.walk_path(root, std::slice::from_ref(ident)) {
                return format!("{}\n\n{line}", field_hover(f));
            }
        }
        line
    }

    /// A model or shape decl's one-line hover, by name.
    fn decl_hover_by_name(&self, name: &str) -> Option<String> {
        self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == name => Some(model_hover(m)),
            Decl::Shape(s) if s.name.node == name => Some(shape_hover(s)),
            Decl::Enum(e) if e.name.node == name => Some(enum_hover(e)),
            _ => None,
        })
    }

    /// An enum variant's hover: `` variant `paid` of `Status` `` (plus its explicit
    /// value, when written). `decl` is the variant's declaration span.
    fn variant_hover(&self, decl: Span) -> Option<String> {
        for d in &self.decls {
            let Decl::Enum(e) = d else { continue };
            for v in &e.variants {
                if v.name.span == decl {
                    let val = match v.value.as_ref().map(|s| &s.node) {
                        Some(VariantValue::Str(s)) => format!(" = \"{s}\""),
                        Some(VariantValue::Int(n)) => format!(" = {n}"),
                        None => String::new(),
                    };
                    return Some(format!(
                        "```based\nvariant {}{val}\n```\nvariant of enum `{}`",
                        v.name.node, e.name.node
                    ));
                }
            }
        }
        None
    }

    /// A scope decl's one-line hover (`scope Name (col: Type = $ctx.field, …)`).
    fn scope_hover(&self, name: &str) -> Option<String> {
        self.decls.iter().find_map(|d| match d {
            Decl::Scope(s) if s.name.node == name => Some(scope_hover(s)),
            _ => None,
        })
    }

    /// Hover for a generated (stored derived) column: `name: <inferred type> = <expr>`.
    /// The type is the one sema inferred from the expression; the expression is
    /// reprinted from the AST. Falls back to just `name = <expr>` if the schema has no
    /// checked type (an unparseable / unchecked buffer).
    fn generated_hover(&self, model: &str, g: &GeneratedField) -> String {
        let expr = based_fmt::reprint_expr(&g.expr);
        let sig = match self.generated_type(model, &g.name.node) {
            Some(ty) => format!("{}: {ty} = {expr}", g.name.node),
            None => format!("{} = {expr}", g.name.node),
        };
        format!(
            "```based\n{sig}\n```\ngenerated column — a stored column derived from the row; \
             read-only (never assigned)"
        )
    }

    /// The inferred type of a generated column (`text`, `decimal(…)`, an enum name, …),
    /// with a trailing `?` when nullable, read from the checked schema. `None` when the
    /// column isn't a checked scalar (no schema, unknown model/field).
    fn generated_type(&self, model: &str, field: &str) -> Option<String> {
        let mem = self.schema.as_ref()?.model(model)?.member(field)?;
        let based_sema::MemberKind::Scalar {
            ty,
            optional,
            enum_name,
            ..
        } = &mem.kind
        else {
            return None;
        };
        let mut s = match enum_name {
            Some(name) => name.clone(),
            None => primitive_str(*ty),
        };
        if *optional {
            s.push('?');
        }
        Some(s)
    }

    /// Hover for a declaration's own name (the cursor sits on the thing being
    /// declared, not a reference to it): the model/field/shape/callable/scope it
    /// introduces.
    fn decl_site_hover(&self, fid: usize, offset: u32) -> Option<String> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for d in &self.decls {
            match d {
                Decl::Model(m) => {
                    if under(&m.name) {
                        return Some(model_hover(m));
                    }
                    for mem in &m.members {
                        match mem {
                            Member::Field(f) if under(&f.name) => return Some(field_hover(f)),
                            Member::Generated(g) if under(&g.name) => {
                                return Some(self.generated_hover(&m.name.node, g));
                            }
                            _ => {}
                        }
                    }
                }
                Decl::Shape(s) if under(&s.name) => return Some(shape_hover(s)),
                Decl::Query(q) if under(&q.name) => return Some(query_hover(q)),
                Decl::Mutation(m) if under(&m.name) => return Some(mutation_hover(m)),
                Decl::Filter(f) if under(&f.name) => return Some(filter_hover(f)),
                Decl::Scope(s) if under(&s.name) => return Some(scope_hover(s)),
                _ => {}
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    /// Hover gives a rust-analyzer-style "what" for the symbol under the cursor: a
    /// field's `name: Type` (+ relation note), and model/shape signatures — for both
    /// references and the declarations themselves.
    #[test]
    fn hover_reports_field_and_decl_signatures() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // The `placed_by: User` field declaration → its signature + relation note.
        let decl = src.find("placed_by:").unwrap() + 1;
        let h = snap.hover_at(fid, decl as u32).expect("field decl hover");
        assert!(h.contains("placed_by: User"), "{h}");
        assert!(h.contains("to-one relation to `User`"), "{h}");

        // A field *reference* in the shape (`placed_by.name`) → the walked field.
        let refoff = src.find("placed_by.name").unwrap() + "placed_by.".len() + 1;
        let hr = snap.hover_at(fid, refoff as u32).expect("field ref hover");
        assert!(hr.contains("name: text"), "{hr}");

        // A model type reference (`items: OrderItem[]`) → `model OrderItem`.
        let mref = src.find("OrderItem[]").unwrap() + 1;
        let hm = snap.hover_at(fid, mref as u32).expect("model ref hover");
        assert!(hm.contains("model OrderItem"), "{hm}");

        // The shape's own name → `shape OrderCard from Order`.
        let sh = src.find("OrderCard from Order").unwrap() + 1;
        let hs = snap.hover_at(fid, sh as u32).expect("shape decl hover");
        assert!(hs.contains("shape OrderCard from Order"), "{hs}");
    }

    /// A `-> stream Shape` query's hover shows the stream return form.
    #[test]
    fn hover_shows_the_stream_return_form() {
        let ws = TempWorkspace::new("stream_hover");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "@sort(total desc)\n\
             Order { status: text  total: int }\n\
             shape OrderRow from Order { status  total }\n\
             query export_orders() -> stream OrderRow;\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;
        let off = (src.find("export_orders").unwrap() + 1) as u32;
        let h = snap.hover_at(fid, off).expect("query hover");
        assert!(h.contains("-> stream OrderRow"), "{h}");
    }

    /// An `-> ok` mutation's hover shows the ack return form as written; the `ok`
    /// token itself is not a type reference (hovering it resolves nothing, no crash).
    #[test]
    fn hover_shows_the_ack_return_form() {
        let ws = TempWorkspace::new("ack_hover");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Tag { label: text }\n\
             mutation drop_tag(id: Id) -> ok { delete Tag where (id = $id) }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;
        let off = (src.find("drop_tag").unwrap() + 1) as u32;
        let h = snap.hover_at(fid, off).expect("mutation hover");
        assert!(h.contains("-> ok"), "{h}");
        let ok_off = (src.find("-> ok").unwrap() + 3) as u32;
        assert!(snap.hover_at(fid, ok_off).is_none());
    }

    /// A generated column (`net = price - discount`) is a real column to the editor:
    /// hover on its declaration shows `name: <inferred type> = <expr>`, dot-completion
    /// through a relation offers it, and it appears as a document symbol.
    #[test]
    fn generated_column_surfaces_in_hover_completion_and_symbols() {
        let ws = TempWorkspace::new("generated_col");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product { id: Id  price: decimal  discount: decimal  net = price - discount }\n\
             Line { id: Id  product: Product }\n\
             shape LineCard from Line { x = product.net }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // Hover on the generated column's declaration: name + inferred type + expr.
        let decl = (src.find("net = price").unwrap() + 1) as u32;
        let h = snap.hover_at(fid, decl).expect("generated column hover");
        assert!(h.contains("net: decimal"), "type not shown: {h}");
        assert!(h.contains("= price - discount"), "expr not shown: {h}");
        assert!(
            h.contains("generated column"),
            "not labelled generated: {h}"
        );

        // Dot-completion through `product.` (root Line → Product) offers `net`.
        let dot = src.find("product.").unwrap() + "product.".len();
        let items = snap.completions(fid, dot as u32);
        assert!(
            items
                .iter()
                .any(|c| c.label == "net" && c.kind == Some(CompletionItemKind::FIELD)),
            "net not offered in completion: {:?}",
            items.iter().map(|c| &c.label).collect::<Vec<_>>()
        );

        // Document symbols nest the generated column as a field child of the model.
        let syms = snap.document_symbols(fid);
        let product = syms.iter().find(|s| s.name == "Product").expect("Product");
        let children = product.children.as_ref().expect("model children");
        assert!(
            children
                .iter()
                .any(|c| c.name == "net" && c.kind == SymbolKind::FIELD),
            "net missing from document symbols"
        );
    }

    /// Hovering a binding states the predicate it generates — on the column/edge
    /// ident (led by the field's signature), on the operator token itself, and on
    /// an unbound param (the derived same-name equality).
    #[test]
    fn binding_hover_states_generated_predicate() {
        let (ws, snap) = binding_snapshot("binding_hover");
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = snap.sources[fid].1.clone();

        // The bound column: field signature + generated predicate.
        let bind_off = (src.find("has tags").unwrap() + "has ".len()) as u32;
        let h = snap.hover_at(fid, bind_off).expect("column hover");
        assert!(h.contains("tags: json"), "{h}");
        assert!(h.contains("binds `tags has $tag`"), "{h}");
        assert!(h.contains("containment (array/json)"), "{h}");

        // The operator token alone still explains the binding.
        let has_off = src.find("has tags").unwrap() as u32;
        let h = snap.hover_at(fid, has_off).expect("operator hover");
        assert!(h.contains("binds `tags has $tag`"), "{h}");

        // An ordered op: the rendered predicate shows the column as left operand.
        let gt_off = src.find("> created_at").unwrap() as u32;
        let h = snap.hover_at(fid, gt_off).expect("ordered-op hover");
        assert!(h.contains("binds `created_at > $since`"), "{h}");
        assert!(h.contains("the column is the left operand"), "{h}");

        // An edge binding: relation signature + the FK equality it generates.
        let edge_off = (src.find("-> author").unwrap() + "-> ".len()) as u32;
        let h = snap.hover_at(fid, edge_off).expect("edge hover");
        assert!(h.contains("author: User"), "{h}");
        assert!(h.contains("binds `author = $user`"), "{h}");

        // An unbound param: the derived same-name equality is discoverable.
        let name_off = (src.find("by_name(name)").unwrap() + "by_name(".len()) as u32;
        let h = snap.hover_at(fid, name_off).expect("unbound param hover");
        assert!(h.contains("name: text"), "{h}");
        assert!(h.contains("binds `name = $name`"), "{h}");
    }
}
