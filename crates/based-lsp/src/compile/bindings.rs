use super::*;


impl Snapshot {
    /// The param-decl name span a cursor resolves to — whether it sits on a callable's
    /// `buyer: Id` param declaration or on a `$buyer` use in that callable's body.
    /// `None` off any param. Params are callable-local, so the returned span identifies
    /// exactly one callable's param.
    pub(super) fn param_ref_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for d in &self.decls {
            let params = decl_params(d);
            if params.is_empty() {
                continue;
            }
            if let Some(p) = params.iter().find(|p| under(&p.name)) {
                return Some(p.name.span);
            }
            for pr in callable_param_refs(d) {
                if pr.path.is_empty() && under(&pr.name) {
                    if let Some(p) = params.iter().find(|p| p.name.node == pr.name.node) {
                        return Some(p.name.span);
                    }
                }
            }
        }
        None
    }


    /// The step-binding decl-name span a cursor resolves to — whether it sits on a
    /// `create … as name` binding declaration or on the `$name` head of a `$name.field`
    /// reference to it. `None` off any binding. Bindings are callable-local, so the span
    /// identifies exactly one mutation's binding.
    pub(super) fn binding_ref_at(&self, fid: usize, offset: u32) -> Option<Span> {
        let under = |id: &Ident| {
            id.span.file.0 as usize == fid && id.span.start <= offset && offset < id.span.end
        };
        for d in &self.decls {
            let binds = callable_binding_decls(d);
            if binds.is_empty() {
                continue;
            }
            if let Some(b) = binds.iter().find(|b| under(b)) {
                return Some(b.span);
            }
            // A `$name.field` head (path non-empty → a step reference, not a bare param)
            // naming one of this callable's bindings.
            for pr in callable_param_refs(d) {
                if !pr.path.is_empty() && under(&pr.name) {
                    if let Some(b) = binds.iter().find(|b| b.node == pr.name.node) {
                        return Some(b.span);
                    }
                }
            }
        }
        None
    }


    /// The canonical occurrence span of the `$ctx` bag field under the cursor, or
    /// `None` off any `$ctx.<field>`. A `$ctx` field is keyed by name (the bag is
    /// coherent across the schema), so every occurrence of one field maps to the same
    /// canonical (its first occurrence in file/offset order).
    pub(super) fn ctx_ref_at(&self, fid: usize, offset: u32) -> Option<Span> {
        for (name, span) in self.ctx_occurrences() {
            if span.file.0 as usize == fid && span.start <= offset && offset < span.end {
                return self.ctx_canonical_span(&name);
            }
        }
        None
    }


    /// Every `$ctx.<field>` occurrence in the project as `(field_name, segment_span)` —
    /// the field segment of each `scope … = $ctx.field` binding and each callable-body
    /// use. The span is the `field` segment (not the `$ctx` prefix), so a rename
    /// rewrites only the bag field name.
    pub(super) fn ctx_occurrences(&self) -> Vec<(String, Span)> {
        let mut out = Vec::new();
        let mut push = |pr: &ParamRef| {
            if pr.name.node == "ctx" {
                if let Some(seg) = pr.path.first() {
                    out.push((seg.node.clone(), seg.span));
                }
            }
        };
        for d in &self.decls {
            if let Decl::Scope(s) = d {
                for t in &s.terms {
                    push(&t.ctx);
                }
            }
            for pr in callable_param_refs(d) {
                push(pr);
            }
        }
        out
    }


    /// The first-in-order occurrence span of `$ctx.<name>` — the rename target every
    /// occurrence of that bag field resolves to.
    pub(super) fn ctx_canonical_span(&self, name: &str) -> Option<Span> {
        self.ctx_occurrences()
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, s)| s)
            .min_by_key(|s| (s.file.0, s.start, s.end))
    }

}
