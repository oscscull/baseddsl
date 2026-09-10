use super::*;


impl Snapshot {
    /// The declaration span of the enum variant the cursor sits on — either a variant
    /// use in value/default position (`where status = paid`, `default pending`) or the
    /// variant's own declaration inside its `enum` body. `None` off any variant. A use is
    /// resolved as a variant only when the compared/assigned column (or the field whose
    /// `default` it is) resolves to an enum type, so a same-named identifier elsewhere is
    /// never mistaken for one; variants are enum-local, so the returned span identifies
    /// exactly one enum's variant.
    pub(super) fn variant_ref_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |sp: Span| sp.file.0 as usize == fid && sp.start <= offset && offset < sp.end;
        // The cursor on a variant declaration resolves to itself.
        for d in &self.decls {
            if let Decl::Enum(e) = d {
                for v in &e.variants {
                    if under(v.name.span) {
                        return Some(v.name.span);
                    }
                }
            }
        }
        // The cursor on a variant use resolves to its declaration.
        for (seg, enum_name) in self.variant_use_sites() {
            if under(seg.span) {
                return self.variant_decl_span(enum_name, &seg.node);
            }
        }
        None
    }


    /// Every enum-variant use site in value/default position, paired with the enum it
    /// belongs to: a `where`/write comparison whose column is enum-typed, a write assign
    /// to an enum column, and an enum field's `default <variant>`. The `Ident` is the
    /// variant token (so its span is the use); the `&str` is the owning enum name.
    pub(super) fn variant_use_sites(&self) -> Vec<(&Ident, &str)> {
        let mut out = Vec::new();
        for d in &self.decls {
            match d {
                Decl::Model(m) => {
                    for mem in &m.members {
                        let Member::Field(f) = mem else { continue };
                        let Some(en) = self.field_enum(f) else {
                            continue;
                        };
                        for md in &f.modifiers {
                            if let Modifier::Default(based_ast::DefaultVal::Variant(v)) = md {
                                out.push((v, en));
                            }
                        }
                    }
                }
                Decl::Query(q) => {
                    let root = match &q.body {
                        QueryBody::Block(stmt) => Some(stmt.model.node.as_str()),
                        QueryBody::Inline(_) => self.query_root(q),
                        QueryBody::Bare | QueryBody::Raw(_) => None,
                    };
                    if let Some(root) = root {
                        match &q.body {
                            QueryBody::Block(stmt) => {
                                for c in &stmt.clauses {
                                    self.clause_variant_sites(c, root, &mut out);
                                }
                            }
                            QueryBody::Inline(cs) => {
                                for c in cs {
                                    self.clause_variant_sites(c, root, &mut out);
                                }
                            }
                            QueryBody::Bare | QueryBody::Raw(_) => {}
                        }
                    }
                }
                Decl::Mutation(m) => self.write_variant_sites(&m.body, &mut out),
                _ => {}
            }
        }
        out
    }


    fn clause_variant_sites<'a>(
        &'a self,
        c: &'a Clause,
        root: &str,
        out: &mut Vec<(&'a Ident, &'a str)>,
    ) {
        if let Clause::Where(p) = c {
            self.pred_variant_sites(p, root, out);
        }
    }


    fn pred_variant_sites<'a>(
        &'a self,
        p: &'a Predicate,
        root: &str,
        out: &mut Vec<(&'a Ident, &'a str)>,
    ) {
        match p {
            Predicate::Or(a, b) | Predicate::And(a, b) => {
                self.pred_variant_sites(a, root, out);
                self.pred_variant_sites(b, root, out);
            }
            Predicate::Not(inner) => self.pred_variant_sites(inner, root, out),
            Predicate::Cmp { path, value, .. } => {
                if let (Some(en), Value::Path(vp)) =
                    (self.enum_of_path(root, &path.segments), value)
                {
                    if vp.segments.len() == 1 {
                        out.push((&vp.segments[0], en));
                    }
                }
            }
            Predicate::InList { path, values } => {
                if let Some(en) = self.enum_of_path(root, &path.segments) {
                    for v in values {
                        if let Value::Path(vp) = v {
                            if vp.segments.len() == 1 {
                                out.push((&vp.segments[0], en));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }


    fn write_variant_sites<'a>(
        &'a self,
        body: &'a [WriteStmt],
        out: &mut Vec<(&'a Ident, &'a str)>,
    ) {
        for w in body {
            match w {
                WriteStmt::Create {
                    model,
                    assigns,
                    from: _,
                    conflict,
                    binding: _,
                } => {
                    self.assign_variant_sites(model.node.as_str(), assigns, out);
                    if let Some(oc) = conflict {
                        self.assign_variant_sites(model.node.as_str(), &oc.update, out);
                    }
                }
                WriteStmt::Update {
                    model,
                    where_,
                    assigns,
                } => {
                    self.pred_variant_sites(where_, model.node.as_str(), out);
                    self.assign_variant_sites(model.node.as_str(), assigns, out);
                }
                WriteStmt::Restore { model, where_ } => {
                    self.pred_variant_sites(where_, model.node.as_str(), out);
                }
                WriteStmt::Delete { model, where_ } | WriteStmt::HardDelete { model, where_ } => {
                    if let Some(p) = where_ {
                        self.pred_variant_sites(p, model.node.as_str(), out);
                    }
                }
                WriteStmt::Tx(inner) => self.write_variant_sites(inner, out),
                WriteStmt::Raw(_) => {}
            }
        }
    }


    fn assign_variant_sites<'a>(
        &'a self,
        model: &str,
        assigns: &'a [Assign],
        out: &mut Vec<(&'a Ident, &'a str)>,
    ) {
        for a in assigns {
            if let (Some(en), Some(Value::Path(vp))) = (
                self.enum_of_path(model, std::slice::from_ref(&a.col)),
                a.value.as_value(),
            ) {
                if vp.segments.len() == 1 {
                    out.push((&vp.segments[0], en));
                }
            }
        }
    }


    /// The enum a dotted column path (rooted at `root`) terminates on, or `None` when the
    /// terminal column is not enum-typed.
    fn enum_of_path<'a>(&'a self, root: &str, segs: &[Ident]) -> Option<&'a str> {
        self.field_enum(self.walk_path(root, segs)?)
    }


    /// The enum name a field is typed by, when its `UpperCamel` type resolves to a
    /// declared enum (not a model relation).
    pub(super) fn field_enum<'a>(&'a self, f: &'a Field) -> Option<&'a str> {
        if let BaseType::Model(t) = &f.ty.base {
            if self.is_enum_decl(&t.node) {
                return Some(t.node.as_str());
            }
        }
        None
    }


    fn is_enum_decl(&self, name: &str) -> bool {
        self.decls
            .iter()
            .any(|d| matches!(d, Decl::Enum(e) if e.name.node == name))
    }


    /// The declaration span of variant `variant` in enum `enum_name`, or `None`.
    fn variant_decl_span(&self, enum_name: &str, variant: &str) -> Option<Span> {
        self.decls.iter().find_map(|d| match d {
            Decl::Enum(e) if e.name.node == enum_name => e
                .variants
                .iter()
                .find(|v| v.name.node == variant)
                .map(|v| v.name.span),
            _ => None,
        })
    }


    /// The `(enum_name, variant_name)` a declaration span identifies, when it is a variant
    /// declaration. Lets find-references key variant uses to the right enum.
    pub(super) fn variant_of_decl_span(&self, target: Span) -> Option<(&str, &str)> {
        for d in &self.decls {
            if let Decl::Enum(e) = d {
                for v in &e.variants {
                    if v.name.span == target {
                        return Some((e.name.node.as_str(), v.name.node.as_str()));
                    }
                }
            }
        }
        None
    }

}
