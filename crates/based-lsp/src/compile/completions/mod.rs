use super::*;
mod items;
use items::*;

impl Snapshot {
    /// Context-aware completions at `(fid, offset)`. The buffer under an edit is
    /// often unparseable, so instead of parsing the half-typed text we read the
    /// source *prefix* before the cursor and classify by its trigger character (the
    /// token immediately before the partial word being typed).
    ///
    /// - `@` → the decorator set.
    /// - `<ident>.` → the fields of the base model, but only when the base is
    ///   statically resolvable (a path rooted in a shape's `from` model or a query
    ///   block's target, walked through relation fields); else nothing — precision
    ///   over recall.
    /// - `:` → primitives + model names (a field's type annotation).
    /// - `->` → model + shape names (a return type).
    /// - anything else → the keyword/function set + model names (the vocabulary
    ///   bucket; works even when the file does not yet parse).
    pub fn completions(&self, fid: usize, offset: u32) -> Vec<CompletionItem> {
        let src = &self.sources[fid].1;
        let before = &src[..(offset as usize).min(src.len())];
        // Drop the partial word under the cursor, then any spaces, to expose the
        // trigger character that classifies the context.
        let head = before
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
            .trim_end();
        match head.chars().next_back() {
            Some('@') => decorator_items(),
            Some('.') => self.field_items_after_dot(fid, offset, head),
            Some(':') => self.type_items(),
            Some('>') if head.ends_with("->") => self.return_type_items(),
            _ => self.default_items(),
        }
    }

    /// Fields of the model a dotted path resolves to, when the base is statically
    /// known. The path is rooted at the enclosing shape's `from` or query block's
    /// target and walked segment-by-segment through relation fields; any segment
    /// that is not a relation (or an unknown root) yields nothing.
    fn field_items_after_dot(&self, fid: usize, offset: u32, head: &str) -> Vec<CompletionItem> {
        let segs = trailing_path(head);
        if segs.is_empty() {
            return Vec::new();
        }
        let Some(root) = self.root_model_at(fid, offset) else {
            return Vec::new();
        };
        let Some(mut model) = self.model_by_name(root) else {
            return Vec::new();
        };
        for seg in &segs {
            let Some(field) = model.members.iter().find_map(|m| match m {
                Member::Field(f) if &f.name.node == seg => Some(f),
                _ => None,
            }) else {
                return Vec::new();
            };
            let BaseType::Model(target) = &field.ty.base else {
                return Vec::new();
            };
            match self.model_by_name(&target.node) {
                Some(m) => model = m,
                None => return Vec::new(),
            }
        }
        model
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Field(f) => Some(item(&f.name.node, CompletionItemKind::FIELD)),
                // A generated column is a real column — offer it like any scalar.
                Member::Generated(g) => Some(item(&g.name.node, CompletionItemKind::FIELD)),
                _ => None,
            })
            .collect()
    }

    /// The model a dotted path is rooted at, if the cursor sits in a decl whose
    /// root is cheaply known: a shape (its `from`) or a query block (its target).
    fn root_model_at(&self, fid: usize, offset: u32) -> Option<&str> {
        let here = |s: Span| s.file.0 as usize == fid && s.start <= offset && offset < s.end;
        self.decls.iter().find_map(|d| match d {
            Decl::Shape(s) if here(s.span) => Some(s.from.node.as_str()),
            Decl::Query(q) if here(q.span) => match &q.body {
                QueryBody::Block(stmt) => Some(stmt.model.node.as_str()),
                _ => None,
            },
            _ => None,
        })
    }

    /// Field type annotation position: the primitives plus every model name.
    fn type_items(&self) -> Vec<CompletionItem> {
        let mut items: Vec<CompletionItem> = PRIMITIVES
            .iter()
            .map(|p| item(p, CompletionItemKind::KEYWORD))
            .collect();
        items.extend(self.model_name_items());
        items
    }

    /// Return type position: models and shapes (a callable may return either), plus
    /// the `stream` return form.
    fn return_type_items(&self) -> Vec<CompletionItem> {
        let mut items = vec![item("stream", CompletionItemKind::KEYWORD)];
        items.extend(self.model_name_items());
        items.extend(self.decls.iter().filter_map(|d| match d {
            Decl::Shape(s) => Some(item(&s.name.node, CompletionItemKind::INTERFACE)),
            _ => None,
        }));
        items
    }

    /// The fallback vocabulary: keywords, functions, and model names.
    fn default_items(&self) -> Vec<CompletionItem> {
        let mut items: Vec<CompletionItem> = KEYWORDS
            .iter()
            .map(|k| item(k, CompletionItemKind::KEYWORD))
            .collect();
        items.extend(
            based_sema::KNOWN_FUNCS
                .iter()
                .chain(based_sema::KNOWN_AGGS)
                .map(|f| item(f, CompletionItemKind::FUNCTION)),
        );
        items.extend(self.model_name_items());
        items
    }

    fn model_name_items(&self) -> Vec<CompletionItem> {
        self.decls
            .iter()
            .filter_map(|d| match d {
                Decl::Model(m) => Some(item(&m.name.node, CompletionItemKind::STRUCT)),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    /// Completion is context-aware off the source prefix: model names in a type
    /// annotation, a base model's fields after a resolvable `.`, the decorator set
    /// after `@`, and the keyword/model vocabulary otherwise. Hermetic over a small
    /// manifest project so the resolution (root model + relation walk) is proven.
    #[test]
    fn completions_by_context() {
        let ws = TempWorkspace::new("completion");
        ws.write("based.toml", "");
        ws.write("org.bsl", "Org { name: text }\n");
        ws.write("user.bsl", "User {\n  org: Org\n  name: text\n}\n");
        ws.write(
            "card.bsl",
            "shape UserCard from User {\n  city = org.name\n}\n",
        );
        ws.write("dec.bsl", "@table(\"things\")\nThing { label: text }\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        let labels = |items: &[CompletionItem]| -> Vec<String> {
            items.iter().map(|c| c.label.clone()).collect()
        };

        // Type position: after `org:` in user.bsl → model names + primitives.
        let ufid = snap.file_id_of(&ws.path("user.bsl")).unwrap();
        let at = snap.sources[ufid].1.find("org:").unwrap() + "org:".len();
        let ty = snap.completions(ufid, at as u32);
        assert!(ty
            .iter()
            .any(|c| c.label == "Org" && c.kind == Some(CompletionItemKind::STRUCT)));
        assert!(labels(&ty).contains(&"text".to_string()));

        // Field after a resolvable `.`: `org.` in the shape (root User, org → Org)
        // completes Org's fields, and *only* fields (precision over recall).
        let cfid = snap.file_id_of(&ws.path("card.bsl")).unwrap();
        let dot = snap.sources[cfid].1.find("org.").unwrap() + "org.".len();
        let fields = snap.completions(cfid, dot as u32);
        assert!(fields
            .iter()
            .any(|c| c.label == "name" && c.kind == Some(CompletionItemKind::FIELD)));
        assert!(fields
            .iter()
            .all(|c| c.kind == Some(CompletionItemKind::FIELD)));

        // A non-resolvable base yields nothing, not wrong suggestions: an overlay
        // whose path steps through a scalar (`name.`) has no model to walk into.
        let mut ov = HashMap::new();
        ov.insert(
            canon(&ws.path("card.bsl")),
            "shape UserCard from User {\n  x = name.\n}\n".to_string(),
        );
        let osnap = compile_manifest(&ws.root, &ov);
        let ofid = osnap.file_id_of(&ws.path("card.bsl")).unwrap();
        let sdot = osnap.sources[ofid].1.find("name.").unwrap() + "name.".len();
        assert!(osnap.completions(ofid, sdot as u32).is_empty());

        // Decorator set after `@`.
        let dfid = snap.file_id_of(&ws.path("dec.bsl")).unwrap();
        let atpos = snap.sources[dfid].1.find("@table").unwrap() + 1;
        let decos = snap.completions(dfid, atpos as u32);
        for want in ["soft_delete", "scope", "table", "index", "was"] {
            assert!(
                labels(&decos).contains(&want.to_string()),
                "decorator {want}"
            );
        }
        assert!(decos
            .iter()
            .all(|c| c.kind == Some(CompletionItemKind::PROPERTY)));

        // Vocabulary bucket at a blank position: keywords + model names.
        let kw = snap.completions(ufid, 0);
        assert!(kw
            .iter()
            .any(|c| c.label == "query" && c.kind == Some(CompletionItemKind::KEYWORD)));
        assert!(labels(&kw).contains(&"User".to_string()));
        assert!(labels(&kw).contains(&"Thing".to_string()));
    }
}
