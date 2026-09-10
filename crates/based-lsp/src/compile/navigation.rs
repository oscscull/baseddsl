use super::*;


impl Snapshot {
    /// Resolve a model/type reference under the cursor to the span of the matching
    /// declaration's name. `(fid, offset)` is the byte offset within file `fid`.
    /// Returns the name span of the `Model` (or `Shape`) the reference names — a
    /// `Location` the editor jumps to — or `None` if the cursor is not on a type
    /// reference, or the referenced type is undeclared in this project.
    pub fn definition_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |id: &&Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        // A model/shape type reference → that decl's name.
        if let Some(id) = collect_type_refs(&self.decls).into_iter().find(under) {
            if let Some(s) = self.type_ref_target(id) {
                return Some(s);
            }
        }
        // A `@scope Name` / `scoped Name` reference → the `scope Name (…)` decl's name.
        if let Some(id) = collect_scope_refs(&self.decls).into_iter().find(under) {
            if let Some(s) = self.scope_ref_target(id) {
                return Some(s);
            }
        }
        // A `filter(...)` call → the `filter Name(…)` decl's name.
        if let Some(id) = collect_filter_refs(&self.decls).into_iter().find(under) {
            if let Some(s) = self.filter_ref_target(id) {
                return Some(s);
            }
        }
        // An enum variant in value/default position (`where status = paid`, `default
        // pending`) → the variant's declaration inside its `enum`. Tried before field
        // references so a variant is never misread as a same-named column.
        if let Some(s) = self.variant_ref_at(fid, offset) {
            return Some(s);
        }
        // A field-reference path segment (`placed_by`, `placed_by.name`, a `where`/
        // `order`/write-assign column) → the field it names, walked through relations
        // from the statically-known root.
        if let Some(f) = self.field_ref_at(fid, offset) {
            return Some(f.name.span);
        }
        // A callable param (`buyer: Id` decl or a `$buyer` use in its body) → the param
        // decl's name. Params are callable-local, so the target span scopes the rename.
        if let Some(s) = self.param_ref_at(fid, offset) {
            return Some(s);
        }
        // A `tx` step binding (`create … as user` decl or a `$user.field` reference) →
        // the binding decl's name. Callable-local, like a param.
        if let Some(s) = self.binding_ref_at(fid, offset) {
            return Some(s);
        }
        // A `$ctx.<field>` bag field (a callable use or a `scope … = $ctx.field` term) →
        // the field's canonical occurrence. The `$ctx` bag is coherent by name across
        // the schema, so one field renames everywhere it is used.
        if let Some(s) = self.ctx_ref_at(fid, offset) {
            return Some(s);
        }
        // A declaration's own name resolves to itself. Conventional for go-to-def on a
        // definition, and load-bearing for the inverse inlay: VS Code activates a label
        // part's `location` by running go-to-def *at* it (LSP 3.17), and that location
        // is the forward edge's declaration — so it must resolve, or the click is inert.
        self.decl_name_at(fid, offset)
    }


    /// The declaration-name span a model/shape/enum type reference names, if declared here.
    fn type_ref_target(&self, id: &Ident) -> Option<Span> {
        self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == id.node => Some(m.name.span),
            Decl::Shape(s) if s.name.node == id.node => Some(s.name.span),
            Decl::Enum(e) if e.name.node == id.node => Some(e.name.span),
            _ => None,
        })
    }


    /// The `scope` decl-name span a `@scope`/`scoped` reference names.
    fn scope_ref_target(&self, id: &Ident) -> Option<Span> {
        self.decls.iter().find_map(|d| match d {
            Decl::Scope(s) if s.name.node == id.node => Some(s.name.span),
            _ => None,
        })
    }


    /// The `filter` decl-name span a `filter(...)` call names.
    fn filter_ref_target(&self, id: &Ident) -> Option<Span> {
        self.decls.iter().find_map(|d| match d {
            Decl::Filter(f) if f.name.node == id.node => Some(f.name.span),
            _ => None,
        })
    }


    /// Every reference site that resolves to the same declaration as the symbol under
    /// the cursor — the inverse of `definition_at`. Powers find-references and (later)
    /// rename. Covers model/shape type references, `@scope`/`scoped` references, filter
    /// calls, field-reference path segments (walked through relations), and the inverse
    /// back-edges that pair through a forward field (so a forward edge's references
    /// include the `Model[]` inverse that joins through it — the "back-follow"). With
    /// `include_decl`, the declaration's own name is included. Deduped, span-ordered.
    pub fn references_at(&self, fid: usize, offset: u32, include_decl: bool) -> Vec<Span> {
        let Some(target) = self.definition_at(fid, offset) else {
            return Vec::new();
        };
        let mut out = Vec::new();

        // Model / shape type references naming the same declaration.
        for id in collect_type_refs(&self.decls) {
            if self.type_ref_target(id) == Some(target) {
                out.push(id.span);
            }
        }
        // Scope references (`@scope` / `scoped`).
        for id in collect_scope_refs(&self.decls) {
            if self.scope_ref_target(id) == Some(target) {
                out.push(id.span);
            }
        }
        // Filter calls.
        for id in collect_filter_refs(&self.decls) {
            if self.filter_ref_target(id) == Some(target) {
                out.push(id.span);
            }
        }
        // Enum variant uses: when the target is a variant declaration, every value/
        // default-position use of that variant. Enum-local — a same-named variant in a
        // different enum is keyed by its own enum, so it is left untouched.
        if let Some((enum_name, variant)) = self.variant_of_decl_span(target) {
            for (seg, en) in self.variant_use_sites() {
                if en == enum_name && seg.node == variant {
                    out.push(seg.span);
                }
            }
        }
        // Field-reference path segments resolving to the target field.
        for (root, segs) in self.field_paths() {
            for (i, seg) in segs.iter().enumerate() {
                if self.walk_path(root, &segs[..=i]).map(|f| f.name.span) == Some(target) {
                    out.push(seg.span);
                }
            }
        }
        // Inverse back-edges: an inferred inverse's `nav` is its paired forward edge,
        // so a forward field's references include the `Model[]` inverse joining through
        // it. (Explicit `(Model.field)` inverses are picked up as field refs below.)
        for f in &self.facts {
            if f.nav == Some(target) {
                out.push(f.span);
            }
        }
        // Explicit inverse pairings `(Model.field)`: the `field` part references it.
        for id in collect_explicit_inverse_fields(&self.decls) {
            if self.explicit_inverse_target(id.0, id.1) == Some(target) {
                out.push(id.1.span);
            }
        }
        // Callable param uses: every `$param` in the callable owning the target param.
        for d in &self.decls {
            if let Some(p) = decl_params(d).iter().find(|p| p.name.span == target) {
                for pr in callable_param_refs(d) {
                    if pr.path.is_empty() && pr.name.node == p.name.node {
                        out.push(pr.name.span);
                    }
                }
            }
        }
        // Step-binding uses: every `$name.field` reference to the `create … as name`
        // binding under the cursor, within its owning callable (bindings are local).
        for d in &self.decls {
            if let Some(b) = callable_binding_decls(d)
                .into_iter()
                .find(|b| b.span == target)
            {
                for pr in callable_param_refs(d) {
                    if !pr.path.is_empty() && pr.name.node == b.node {
                        out.push(pr.name.span);
                    }
                }
            }
        }
        // `$ctx.<field>` uses: every occurrence of the bag field whose canonical is the
        // target (the scope-term binding and every callable use share one name).
        let ctx = self.ctx_occurrences();
        if let Some(name) = ctx
            .iter()
            .map(|(n, _)| n)
            .find(|n| self.ctx_canonical_span(n) == Some(target))
            .cloned()
        {
            for (n, span) in &ctx {
                if *n == name {
                    out.push(*span);
                }
            }
        }

        if include_decl {
            out.push(target);
        }
        out.sort_by_key(|s| (s.file.0, s.start, s.end));
        out.dedup();
        out
    }


    /// Resolve an explicit inverse's `(Model.field)` to that field's name span.
    fn explicit_inverse_target(&self, model: &Ident, field: &Ident) -> Option<Span> {
        let m = self.model_by_name(&model.node)?;
        m.members.iter().find_map(|mem| match mem {
            Member::Field(f) if f.name.node == field.node => Some(f.name.span),
            _ => None,
        })
    }


    /// The name span of a declaration whose own name the cursor sits on: a model, one
    /// of its fields, a shape, a query/mutation/filter, or a scope. `None` elsewhere.
    fn decl_name_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for d in &self.decls {
            match d {
                Decl::Model(m) => {
                    if under(&m.name) {
                        return Some(m.name.span);
                    }
                    for mem in &m.members {
                        if let Member::Field(f) = mem {
                            if under(&f.name) {
                                return Some(f.name.span);
                            }
                        }
                    }
                }
                Decl::Shape(s) if under(&s.name) => return Some(s.name.span),
                Decl::Query(q) if under(&q.name) => return Some(q.name.span),
                Decl::Mutation(m) if under(&m.name) => return Some(m.name.span),
                Decl::Filter(f) if under(&f.name) => return Some(f.name.span),
                Decl::Scope(s) if under(&s.name) => return Some(s.name.span),
                Decl::Enum(e) if under(&e.name) => return Some(e.name.span),
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

    /// A cursor inside a model *reference* (`org: Org`) resolves to that model's
    /// declaration span in the sibling file that declares it — the go-to-definition
    /// core, hermetic over a two-file manifest project.
    #[test]
    fn goto_definition_resolves_model_reference_cross_file() {
        let ws = TempWorkspace::new("gotodef");
        ws.write("based.toml", "");
        ws.write("org.bsl", "Org { name: text }\n");
        ws.write("user.bsl", "User {\n  org: Org\n  name: text\n}\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        // Cursor mid-`Org` in the `org: Org` field type of user.bsl.
        let user_fid = snap.file_id_of(&ws.path("user.bsl")).unwrap();
        let src = &snap.sources[user_fid].1;
        let off = (src.find("Org").unwrap() + 1) as u32;
        let def = snap
            .definition_at(user_fid, off)
            .expect("reference resolves to a declaration");

        // It points at the `Org` model's name span, in org.bsl.
        let (def_path, def_src) = &snap.sources[def.file.0 as usize];
        assert!(def_path.ends_with("org.bsl"), "{def_path:?}");
        assert_eq!(&def_src[def.start as usize..def.end as usize], "Org");

        // Whitespace (a non-reference offset) resolves to nothing.
        let ws_off = src.find("\n  name").unwrap() as u32;
        assert_eq!(snap.definition_at(user_fid, ws_off), None);
    }


    /// A `field -> Shape` nest reference resolves to the referenced shape decl's
    /// name span (cross-file), and the decl's references include the nest site.
    #[test]
    fn goto_definition_resolves_shape_nest_reference() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        // Cursor mid-`UserRef` in `placed_by -> UserRef` (order/model.bsl). The last
        // occurrence is the shape field (earlier ones sit in comments).
        let ofid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[ofid].1;
        let off = (src.rfind("-> UserRef").unwrap() + 4) as u32;
        let def = snap
            .definition_at(ofid, off)
            .expect("shape reference resolves");
        let (def_path, def_src) = &snap.sources[def.file.0 as usize];
        assert!(def_path.ends_with("user/model.bsl"), "{def_path:?}");
        assert_eq!(&def_src[def.start as usize..def.end as usize], "UserRef");

        // Find-references from the decl lists the nest reference site.
        let ufid = def.file.0 as usize;
        let refs = snap.references_at(ufid, def.start, false);
        assert!(
            refs.iter().any(|s| s.file.0 as usize == ofid),
            "nest reference listed: {refs:?}"
        );
    }


    /// Go-to-definition from a `@scope Name` (model) or `scoped Name` (callable)
    /// reference resolves to the `scope Name (…)` decl's name span — the both-sides
    /// scope contract is navigable from either reference.
    #[test]
    fn goto_definition_resolves_scope_reference() {
        let ws = TempWorkspace::new("gotoscope");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "scope Tenant (org: Org = $ctx.org)\n\
             Org { name: text }\n\
             @scope Tenant\n\
             Widget { org: Org  name: text }\n\
             query widgets() -> Widget[] scoped Tenant { list Widget; }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // The `scope Tenant` decl's own name span (the definition target).
        let decl_at = src.find("scope Tenant").unwrap() + "scope ".len();
        let def_span = src[decl_at..decl_at + "Tenant".len()].to_string();
        assert_eq!(def_span, "Tenant");

        // From the `@scope Tenant` reference on the model.
        let deco_ref = (src.find("@scope Tenant").unwrap() + "@scope ".len() + 1) as u32;
        let d1 = snap.definition_at(fid, deco_ref).expect("@scope resolves");
        assert_eq!(&src[d1.start as usize..d1.end as usize], "Tenant");
        assert_eq!(d1.start as usize, decl_at);

        // From the `scoped Tenant` reference on the query.
        let scoped_ref = (src.find("scoped Tenant").unwrap() + "scoped ".len() + 1) as u32;
        let d2 = snap
            .definition_at(fid, scoped_ref)
            .expect("scoped resolves");
        assert_eq!(&src[d2.start as usize..d2.end as usize], "Tenant");
        assert_eq!(d2.start as usize, decl_at);
    }


    /// Find-references on a forward edge includes the inverse that pairs through it —
    /// the "back-follow". `OrderItem.order`'s references include `Order.items` (the
    /// inferred inverse joining via `order`), plus the declaration itself when asked.
    #[test]
    fn references_include_inverse_back_edge() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        // Cursor on the `order` forward-edge declaration in order_item/model.bsl.
        let oi_fid = snap.file_id_of(&root.join("order_item/model.bsl")).unwrap();
        let oi_src = &snap.sources[oi_fid].1;
        let off = (oi_src.find("order:").unwrap() + 1) as u32;

        let refs = snap.references_at(oi_fid, off, true);
        // The inverse `Order.items` (in order/model.bsl) is among the references.
        let model_fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let model_src = &snap.sources[model_fid].1;
        assert!(
            refs.iter().any(|s| s.file.0 as usize == model_fid
                && &model_src[s.start as usize..s.end as usize] == "items"),
            "inverse back-edge `Order.items` should be a reference: {refs:?}"
        );
        // With include_declaration, the `order` field's own name is present too.
        assert!(
            refs.iter().any(|s| s.file.0 as usize == oi_fid
                && &oi_src[s.start as usize..s.end as usize] == "order"),
            "the declaration itself: {refs:?}"
        );
    }


    /// Find-references + go-to-def reach into query/mutation bodies: a field used in a
    /// query `where` and a `filter(...)` call are both resolved.
    #[test]
    fn references_reach_query_bodies_and_filters() {
        let ws = TempWorkspace::new("refs");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget { active: bool  qty: int? }\n\
             filter big(n: int) = qty > $n;\n\
             shape W from Widget { active }\n\
             query find() -> W[] { list Widget where (active and big(5)); }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // Go-to-def on the `big(5)` filter call → the `filter big` declaration.
        let call = (src.find("big(5)").unwrap() + 1) as u32;
        let def = snap.definition_at(fid, call).expect("filter call resolves");
        assert_eq!(&src[def.start as usize..def.end as usize], "big");
        assert_eq!(def.start as usize, src.find("big(n").unwrap());

        // Find-references on the `filter big` declaration → the call site.
        let decl = (src.find("big(n").unwrap() + 1) as u32;
        let frefs = snap.references_at(fid, decl, false);
        assert!(
            frefs
                .iter()
                .any(|s| s.start as usize == src.find("big(5)").unwrap()),
            "the filter call site: {frefs:?}"
        );

        // Find-references on the `active` field → its use in the query `where`.
        let afield = (src.find("active: bool").unwrap() + 1) as u32;
        let arefs = snap.references_at(fid, afield, false);
        assert!(
            arefs
                .iter()
                .any(|s| s.start as usize == src.find("active and").unwrap()),
            "the `where` use of active: {arefs:?}"
        );
    }


    /// Go-to-definition on a *field-reference* path resolves each segment to the
    /// field it names, walking through relations from the shape's `from` root — even
    /// when the walk crosses into another file's model (`placed_by.name` → `User.name`).
    #[test]
    fn goto_definition_resolves_field_reference_path() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        // `buyer = placed_by.name` in OrderCard (order/model.bsl).
        let model_fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();
        let src = &snap.sources[model_fid].1;
        let base = src.find("placed_by.name").unwrap();

        // Cursor on `placed_by` → the `Order.placed_by` field decl, same file.
        let def = snap
            .definition_at(model_fid, (base + 1) as u32)
            .expect("placed_by resolves");
        let (dp, ds) = &snap.sources[def.file.0 as usize];
        assert!(dp.ends_with("order/model.bsl"), "{dp:?}");
        assert_eq!(&ds[def.start as usize..def.end as usize], "placed_by");

        // Cursor on the trailing `.name` → the `User.name` field, in user/model.bsl.
        let name_off = base + "placed_by.".len() + 1;
        let ndef = snap
            .definition_at(model_fid, name_off as u32)
            .expect("name resolves through the relation");
        let (np, nsrc) = &snap.sources[ndef.file.0 as usize];
        assert!(np.ends_with("user/model.bsl"), "{np:?}");
        assert_eq!(&nsrc[ndef.start as usize..ndef.end as usize], "name");

        // A bare shape field (`status`) resolves to the local column, too.
        let st = src.find("\n  status\n").map(|p| p + 3).unwrap();
        let sdef = snap.definition_at(model_fid, st as u32).unwrap();
        assert_eq!(
            &src[sdef.start as usize..sdef.end as usize],
            "status",
            "bare shape field resolves to its column"
        );
    }


    /// Field-reference go-to-def reaches beyond shapes: a query block's `where`/`order`
    /// columns and a mutation's create-assign columns all resolve to the model field,
    /// rooted at the statement target / write model.
    #[test]
    fn goto_definition_resolves_query_and_mutation_columns() {
        let ws = TempWorkspace::new("colrefs");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget { label: text  qty: int? }\n\
             shape W from Widget { label }\n\
             query find() -> W[] { list Widget where (qty > 0) order (label asc); }\n\
             mutation add(l: text) -> W { create Widget { label = $l }; }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;
        let label_decl = src.find("label: text").unwrap();

        // `where (qty > 0)` → the `qty` column.
        let qty = src.find("qty > 0").unwrap() + 1;
        let d = snap.definition_at(fid, qty as u32).expect("where column");
        assert_eq!(&src[d.start as usize..d.end as usize], "qty");

        // `order (label asc)` → the `label` column (its declaration).
        let ord = src.find("label asc").unwrap() + 1;
        let d = snap.definition_at(fid, ord as u32).expect("order column");
        assert_eq!(d.start as usize, label_decl);

        // `create Widget { label = $l }` → the `label` column.
        let asg = src.find("label = $l").unwrap() + 1;
        let d = snap.definition_at(fid, asg as u32).expect("assign column");
        assert_eq!(d.start as usize, label_decl);
    }


    #[test]
    fn goto_def_on_a_variant_use_resolves_to_its_declaration() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        // The declaration span of `paid` in `enum Status`.
        let decl = src.find("pending, paid, shipped").unwrap() + "pending, ".len();

        // `where (status = paid)` → the `paid` variant declaration.
        let use_off = (src.find("status = paid").unwrap() + "status = ".len()) as u32;
        let d = snap
            .definition_at(fid, use_off)
            .expect("variant use resolves");
        assert_eq!(
            d.start as usize,
            decl,
            "{}",
            &src[d.start as usize..d.end as usize]
        );

        // `default pending` → the `pending` variant declaration.
        let pend_decl = src.find("pending, paid").unwrap();
        let def_off = (src.find("default pending").unwrap() + "default ".len()) as u32;
        let d = snap
            .definition_at(fid, def_off)
            .expect("default variant resolves");
        assert_eq!(d.start as usize, pend_decl);

        // The write assign `status = shipped` → the `shipped` variant.
        let ship_decl = src.find("paid, shipped").unwrap() + "paid, ".len();
        let asg = (src.find("status = shipped").unwrap() + "status = ".len()) as u32;
        let d = snap
            .definition_at(fid, asg)
            .expect("assign variant resolves");
        assert_eq!(d.start as usize, ship_decl);
    }


    #[test]
    fn goto_def_on_a_variant_inside_an_in_list_resolves() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        let decl = src.find("pending, paid, shipped").unwrap() + "pending, ".len();
        // `where (status in (paid, shipped))` → the `paid` variant declaration.
        let use_off = (src.find("status in (paid").unwrap() + "status in (".len()) as u32;
        let d = snap
            .definition_at(fid, use_off)
            .expect("in-list variant use resolves");
        assert_eq!(
            d.start as usize,
            decl,
            "{}",
            &src[d.start as usize..d.end as usize]
        );
    }


    #[test]
    fn find_references_on_a_variant_are_enum_local() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        // Cursor on the `pending` declaration in `enum Status`.
        let off = (src.find("pending, paid, shipped").unwrap() + 1) as u32;
        let refs = snap.references_at(fid, off, true);
        // The `Status.pending` decl + the `default pending` use — NOT `Grade.pending`.
        let texts: Vec<&str> = refs
            .iter()
            .map(|s| &src[s.start as usize..s.end as usize])
            .collect();
        assert!(texts.iter().all(|t| *t == "pending"), "{texts:?}");
        assert_eq!(
            refs.len(),
            2,
            "Status.pending decl + one use only: {texts:?}"
        );
        // None of the references is the Grade.pending declaration.
        let grade_pending = src.find("pending, top").unwrap();
        assert!(
            refs.iter().all(|s| s.start as usize != grade_pending),
            "Grade.pending must be untouched"
        );
    }


    #[test]
    fn enum_type_ref_goto_def_and_hover_still_work() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        // `status: Status` type reference → the `enum Status` declaration name.
        let ty_ref = (src.find("status: Status").unwrap() + "status: ".len()) as u32;
        let d = snap
            .definition_at(fid, ty_ref)
            .expect("enum type ref resolves");
        let enum_decl = src.find("Status {").unwrap();
        assert_eq!(d.start as usize, enum_decl);
        // Hover on the enum type reference names the enum.
        let h = snap.hover_at(fid, ty_ref).expect("enum hover");
        assert!(h.contains("enum Status"), "{h}");
        // Hover on a variant use names its enum.
        let vh = (src.find("status = paid").unwrap() + "status = ".len()) as u32;
        let h = snap.hover_at(fid, vh).expect("variant hover");
        assert!(h.contains("variant of enum `Status`"), "{h}");
    }


    /// A signature binding's column/edge ident is a field reference like any other:
    /// go-to-def resolves it, find-references lists it, and renaming the field
    /// rewrites it (the NF-observed hole: a rename that skipped `has tags` silently
    /// broke the schema).
    #[test]
    fn binding_column_navigates_and_renames() {
        let (ws, snap) = binding_snapshot("binding_nav");
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = snap.sources[fid].1.clone();

        // Go-to-def on `tags` in `has tags` → the field declaration.
        let bind_off = (src.find("has tags").unwrap() + "has ".len()) as u32;
        let def = snap
            .definition_at(fid, bind_off)
            .expect("binding column resolves");
        assert_eq!(def.start as usize, src.find("tags: json").unwrap());

        // Go-to-def on `author` in `user -> author` → the relation declaration.
        let edge_off = (src.find("-> author").unwrap() + "-> ".len()) as u32;
        let edge_def = snap
            .definition_at(fid, edge_off)
            .expect("binding edge resolves");
        assert_eq!(edge_def.start as usize, src.find("author: User").unwrap());

        // Find-references from the field decl lists the binding site.
        let refs = snap.references_at(fid, def.start, false);
        assert!(
            refs.iter().any(|s| s.start == bind_off),
            "binding site listed: {refs:?}"
        );

        // Renaming the field rewrites its declaration AND the binding use.
        let changes = snap
            .rename_edits(fid, def.start, "labels")
            .expect("field is renameable");
        let texts = rename_texts(&snap, &changes);
        assert_eq!(texts.len(), 2, "decl + binding: {texts:?}");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        assert!(out.contains("labels: json"), "{out}");
        assert!(out.contains("tag: json has labels"), "{out}");
    }


    /// A raw-bodied query is inert editor surface: hover walks it without crashing,
    /// find-references lists a `${param}` use (at the raw block), and renaming the
    /// param rewrites only sites literally spelling the name — the opaque raw text
    /// is never corrupted.
    #[test]
    fn raw_query_body_is_inert_but_param_refs_resolve() {
        let ws = TempWorkspace::new("raw_query_body");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "User { name: text, total: int }\n\
             shape UserRow from User { name }\n\
             query heavy(min: int) -> UserRow[] {\n\
               raw`SELECT name FROM user WHERE total >= ${min}`;\n\
             }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // Hover across the whole raw line never panics.
        let line_start = src.find("raw`").unwrap() as u32;
        let line_end = src.find("`;").unwrap() as u32 + 1;
        for off in line_start..line_end {
            let _ = snap.hover_at(fid, off);
        }

        // The `${min}` use references the declared param (the raw block anchors it).
        let decl_off = (src.find("heavy(min").unwrap() + "heavy(".len()) as u32;
        let refs = snap.references_at(fid, decl_off, false);
        assert!(!refs.is_empty(), "raw `${{min}}` use should be listed");

        // Rename rewrites the decl only — the raw text spells `${min}` inside a
        // block whose span text is not `min`, so it is left alone (the miss is a
        // a loud error on recompile, not silent corruption).
        let changes = snap
            .rename_edits(fid, decl_off, "floor")
            .expect("param is renameable");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        assert!(out.contains("query heavy(floor: int)"), "{out}");
        assert!(out.contains("${min}"), "raw text must be untouched: {out}");
    }


    /// A `tx` step binding (`create … as name`) is a first-class editor symbol: the
    /// `$name.field` head resolves to the `as name` decl (go-to-def), find-references
    /// lists the use, rename rewrites the decl + every use, and hover names its model.
    #[test]
    fn tx_step_binding_navigates_and_renames() {
        let ws = TempWorkspace::new("tx_binding");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "User { name: text }\n\
             Address { user: User, city: text }\n\
             shape UserRow from User { name }\n\
             mutation signup(city: text) -> UserRow {\n\
               tx {\n\
                 create User { name = \"x\" } as user;\n\
                 create Address { user = $user.id, city = $city };\n\
               }\n\
             }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // Go-to-def on the `$user` head resolves to the `as user` binding decl.
        let use_off = (src.find("$user.id").unwrap() + 1) as u32;
        let def = snap.definition_at(fid, use_off).expect("$user resolves");
        let decl_off = (src.find("as user").unwrap() + "as ".len()) as u32;
        let decl = snap
            .definition_at(fid, decl_off)
            .expect("binding decl resolves to itself");
        assert_eq!(
            def, decl,
            "the `$user` head points at its `as user` binding"
        );

        // Find-references from the decl lists the `$user.id` use.
        let refs = snap.references_at(fid, decl_off, false);
        assert!(!refs.is_empty(), "the `$user.id` use should be listed");

        // Hover on the binding names the model of the row it binds.
        let hov = snap.hover_at(fid, decl_off).expect("binding hover");
        assert!(hov.contains("User"), "{hov}");

        // Rename `user` -> `owner` rewrites the decl and the use, not `$city`.
        let changes = snap
            .rename_edits(fid, decl_off, "owner")
            .expect("binding is renameable");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        assert!(out.contains("} as owner;"), "{out}");
        assert!(out.contains("user = $owner.id"), "{out}");
        assert!(out.contains("city = $city"), "$city untouched: {out}");
    }

}
