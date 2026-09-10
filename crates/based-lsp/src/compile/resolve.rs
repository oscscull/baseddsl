use super::*;

impl Snapshot {
    /// The field a reference path segment under the cursor names. Every path in a
    /// shape body / query clause / mutation write is rooted at a statically-known
    /// model (the shape's `from`, the statement target, the write model) and walked
    /// segment-by-segment through relation edges; the segment the cursor is on
    /// resolves to its declaring field. `None` off any such segment.
    pub(super) fn field_ref_at(&self, fid: usize, offset: u32) -> Option<&Field> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for (root, segs) in self.field_paths() {
            if let Some(i) = segs.iter().position(under) {
                return self.walk_path(root, &segs[..=i]);
            }
        }
        None
    }

    /// Resolve a path prefix against `root`, returning the field its last segment
    /// names. Intermediate segments must be relation edges (they advance the model);
    /// the final segment may be a scalar or relation. `None` if any segment is not a
    /// field of the model reached so far.
    pub(super) fn walk_path(&self, root: &str, segs: &[Ident]) -> Option<&Field> {
        let mut model = self.model_by_name(root)?;
        let mut last: Option<&Field> = None;
        for seg in segs {
            let field = model.members.iter().find_map(|m| match m {
                Member::Field(f) if f.name.node == seg.node => Some(f),
                _ => None,
            })?;
            last = Some(field);
            if let BaseType::Model(t) = &field.ty.base {
                match self.model_by_name(&t.node) {
                    Some(m) => model = m,
                    None => break,
                }
            }
        }
        last
    }

    /// Every field-reference path in the project, each paired with the model it is
    /// rooted at. Covers shape bodies, query `where`/`order` clauses, signature
    /// param bindings (`-> edge` / `op col`), and mutation write `where`/assign
    /// columns — the contexts whose root model is statically known. (Filters are
    /// omitted: their root is the polymorphic call site.)
    pub(super) fn field_paths(&self) -> Vec<(&str, &[Ident])> {
        let mut out = Vec::new();
        for d in &self.decls {
            match d {
                Decl::Shape(s) => self.shape_paths(s.from.node.as_str(), &s.body, &mut out),
                Decl::Query(q) => {
                    // Clauses and bindings both root at the query's target model:
                    // the explicit statement target of a block, else the inferred
                    // target (its return).
                    let root = match &q.body {
                        QueryBody::Block(stmt) => Some(stmt.model.node.as_str()),
                        _ => self.query_root(q),
                    };
                    let Some(root) = root else { continue };
                    for p in &q.params {
                        match &p.binding {
                            Some(ParamBinding::Edge(id) | ParamBinding::ColOp { col: id, .. }) => {
                                out.push((root, std::slice::from_ref(id)));
                            }
                            None => {}
                        }
                    }
                    match &q.body {
                        QueryBody::Block(stmt) => {
                            for c in &stmt.clauses {
                                clause_paths(c, root, &mut out);
                            }
                        }
                        QueryBody::Inline(clauses) => {
                            for c in clauses {
                                clause_paths(c, root, &mut out);
                            }
                        }
                        // A raw body is opaque SQL — no field paths inside.
                        QueryBody::Bare | QueryBody::Raw(_) => {}
                    }
                }
                Decl::Mutation(m) => write_paths(&m.body, &mut out),
                _ => {}
            }
        }
        out
    }

    /// Collect a shape body's field paths (rooted at `from`), recursing into `field {
    /// … }` sub-objects against the relation's target model.
    fn shape_paths<'a>(
        &'a self,
        from: &'a str,
        body: &'a [ShapeField],
        out: &mut Vec<(&'a str, &'a [Ident])>,
    ) {
        for sf in body {
            match sf {
                ShapeField::Bare(id) => out.push((from, std::slice::from_ref(id))),
                ShapeField::Rename {
                    value: ShapeValue::Path(p),
                    ..
                } => out.push((from, &p.segments)),
                // An aggregate's argument column is a field reference (navigable /
                // renamable); a raw-SQL value has no field path.
                ShapeField::Rename {
                    value: ShapeValue::Agg(AggCall { arg: Some(p), .. }),
                    ..
                } => out.push((from, &p.segments)),
                // A computed field's operand columns (and CASE `when` comparisons) are all
                // navigable / renamable field references, rooted at `from`.
                ShapeField::Rename {
                    value: ShapeValue::Computed(expr),
                    ..
                } => computed_paths(expr, from, out),
                ShapeField::Rename { .. } => {}
                ShapeField::Nest { field, body } => {
                    out.push((from, std::slice::from_ref(field)));
                    if let Some(target) = self.relation_target(from, &field.node) {
                        self.shape_paths(target, body, out);
                    }
                }
                // `field -> Shape`: the field is a relation reference; the shape name
                // is a type reference (collect_type_refs), not a field path.
                ShapeField::NestRef { field, .. } => {
                    out.push((from, std::slice::from_ref(field)));
                }
                // `out = edge.far { body }`: the path segments are navigable relation
                // references (rooted at `from`); the body reaches the far model.
                ShapeField::Flatten { path, body, .. } => {
                    out.push((from, &path.segments));
                    let far = path
                        .segments
                        .iter()
                        .try_fold(from, |cur, seg| self.relation_target(cur, &seg.node));
                    if let Some(far) = far {
                        self.shape_paths(far, body, out);
                    }
                }
                // `...Base` is a shape-name (type) reference, not a field path —
                // navigation to it is handled by `collect_type_refs`.
                ShapeField::Spread { .. } => {}
            }
        }
    }

    /// The model a relation `field` on `model` points at, if it is a relation edge.
    fn relation_target(&self, model: &str, field: &str) -> Option<&str> {
        let m = self.model_by_name(model)?;
        m.members.iter().find_map(|mem| match mem {
            Member::Field(f) if f.name.node == field => match &f.ty.base {
                BaseType::Model(t) => Some(t.node.as_str()),
                _ => None,
            },
            _ => None,
        })
    }

    /// The model an inline/bare query reads from: its return shape's `from`, or the
    /// return model itself when the return type is a bare model.
    pub(super) fn query_root(&self, q: &Query) -> Option<&str> {
        let ret = q.ret.ty.node.as_str();
        self.decls.iter().find_map(|d| match d {
            Decl::Shape(s) if s.name.node == ret => Some(s.from.node.as_str()),
            Decl::Model(m) if m.name.node == ret => Some(m.name.node.as_str()),
            _ => None,
        })
    }

    // ---- Enum variant navigation ------------------------------------------

    pub(super) fn model_by_name(&self, name: &str) -> Option<&Model> {
        self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == name => Some(m),
            _ => None,
        })
    }
}
