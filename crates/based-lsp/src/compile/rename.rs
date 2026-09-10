use super::*;


/// Whether `s` is a well-formed identifier — a rename target must be one, else the
/// edit would produce unparseable source. (Casing rules, e.g. models UpperName, are
/// left to sema, which re-flags a bad rename inline.)
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}


impl Snapshot {
    /// The workspace edit renaming the symbol under the cursor to `new_name`, grouped
    /// by owning file. One text edit per occurrence that *textually* spells the old
    /// name — so the inverse back-edge (a differently-named field that merely pairs
    /// through the symbol, e.g. `Order.items` for `OrderItem.order`) is left untouched,
    /// unlike find-references which lists it. `None` when the cursor is not on a
    /// renameable symbol or `new_name` is not a valid identifier.
    pub fn rename_edits(
        &self,
        fid: usize,
        offset: u32,
        new_name: &str,
    ) -> Option<HashMap<Url, Vec<TextEdit>>> {
        if !is_ident(new_name) {
            return None;
        }
        let target = self.definition_at(fid, offset)?;
        let old = self.span_text(target)?.to_string();
        let mut edits: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        for span in self.references_at(fid, offset, true) {
            // Rewrite only sites literally spelling the old name; a back-edge pairs
            // through the symbol under a different name and must not be renamed.
            if self.span_text(span) != Some(old.as_str()) {
                continue;
            }
            let f = span.file.0 as usize;
            let Ok(uri) = Url::from_file_path(&self.sources[f].0) else {
                continue;
            };
            edits.entry(uri).or_default().push(TextEdit {
                range: span_range(span, &self.lines[f]),
                new_text: new_name.to_string(),
            });
        }
        // If the renamed symbol maps to a live DB column/table, also insert a `@was`
        // so the generated migration preserves data (rename, not drop+add).
        if let Some((uri, edit)) = self.was_edit_for_rename(target, &old, new_name) {
            edits.entry(uri).or_default().push(edit);
        }
        (!edits.is_empty()).then_some(edits)
    }


    /// The identifier range under the cursor to offer for rename (prepareRename), or
    /// `None` when the cursor is not on a renameable symbol. The range is the extent of
    /// the identifier the cursor sits in; renameability is gated on the same resolver
    /// go-to-def uses, so keywords, literals, and whitespace decline.
    pub fn prepare_rename_range(&self, fid: usize, offset: u32) -> Option<Range> {
        self.definition_at(fid, offset)?;
        let (start, end) = word_extent(&self.sources[fid].1, offset)?;
        let idx = &self.lines[fid];
        Some(Range::new(idx.position(start), idx.position(end)))
    }


    /// The extra edit that makes a field/model rename **data-preserving**: a `@was("old")`
    /// naming the declaration's current physical column/table, so the next generated
    /// migration renames it (keeping data) instead of drop+add. Emitted only when the
    /// rename actually changes the physical name (no `(column …)` / `@table` override
    /// decouples it), the declaration has no `@was` already (an existing one still names
    /// the snapshot's column — a rename chain must keep the original), and the physical
    /// name is a **live** column/table in the project's latest captured snapshot (an
    /// uncaptured column needs no rename step). `None` outside those conditions.
    fn was_edit_for_rename(
        &self,
        target: Span,
        old: &str,
        new_name: &str,
    ) -> Option<(Url, TextEdit)> {
        if old == new_name {
            return None;
        }
        let schema = self.schema.as_ref()?;
        let prev = latest_snapshot(self.migrations_root.as_deref()?)?;
        for d in &self.decls {
            let Decl::Model(m) = d else { continue };
            // Model rename → `@was("old_table")` as a leading decorator line.
            if m.name.span == target {
                if m.decorators.iter().any(|dc| dc.name.node == "was")
                    || m.decorators.iter().any(|dc| dc.name.node == "table")
                {
                    return None;
                }
                let table = &schema.model(&m.name.node)?.table;
                prev.table(table)?;
                let fid = m.name.span.file.0 as usize;
                let at = line_start(&self.sources[fid].1, m.name.span.start);
                return self.insertion(fid, at, format!("@was(\"{table}\")\n"));
            }
            // Field rename → ` @was("old_col")` appended to the field's modifiers.
            for mem in &m.members {
                let Member::Field(f) = mem else { continue };
                if f.name.span != target {
                    continue;
                }
                if f.was.is_some()
                    || f.modifiers
                        .iter()
                        .any(|md| matches!(md, Modifier::Column(_)))
                {
                    return None;
                }
                let rmodel = schema.model(&m.name.node)?;
                let rmem = rmodel.member(&f.name.node)?;
                if matches!(rmem.kind, based_sema::MemberKind::Inverse { .. }) {
                    return None;
                }
                let col = rmem.physical_col().to_string();
                prev.table(&rmodel.table)?.column(&col)?;
                let fid = f.span.file.0 as usize;
                return self.insertion(fid, f.span.end as usize, format!(" @was(\"{col}\")"));
            }
        }
        None
    }


    /// A zero-width `TextEdit` inserting `text` at byte `offset` in file `fid`.
    fn insertion(&self, fid: usize, offset: usize, text: String) -> Option<(Url, TextEdit)> {
        let uri = Url::from_file_path(&self.sources[fid].0).ok()?;
        let pos = self.lines[fid].position(offset);
        Some((
            uri,
            TextEdit {
                range: Range::new(pos, pos),
                new_text: text,
            },
        ))
    }

}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    #[test]
    fn rename_model_rewrites_declaration_and_cross_file_refs() {
        let ws = TempWorkspace::new("rename_model");
        ws.write("based.toml", "");
        ws.write("org.bsl", "Org { name: text }\n");
        ws.write("user.bsl", "User {\n  org: Org\n  name: text\n}\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        // Cursor on the `Org` reference in user.bsl → rename to `Organization`.
        let ufid = snap.file_id_of(&ws.path("user.bsl")).unwrap();
        let off = (snap.sources[ufid].1.find("Org").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(ufid, off, "Organization")
            .expect("model reference is renameable");

        // Both files carry an edit: the decl (org.bsl) and the type ref (user.bsl).
        let ofid = snap.file_id_of(&ws.path("org.bsl")).unwrap();
        let ouri = Url::from_file_path(&snap.sources[ofid].0).unwrap();
        let uuri = Url::from_file_path(&snap.sources[ufid].0).unwrap();
        assert!(changes.contains_key(&ouri), "declaration file edited");
        assert!(changes.contains_key(&uuri), "reference file edited");

        // Every rewritten site spelled the old name `Org` (never `name`, `User`, …).
        let texts = rename_texts(&snap, &changes);
        assert_eq!(texts.len(), 2, "decl + one reference: {texts:?}");
        assert!(texts.iter().all(|t| t == "Org"), "{texts:?}");
        // The new text is the requested name.
        assert!(changes
            .values()
            .flatten()
            .all(|e| e.new_text == "Organization"));
    }


    #[test]
    fn rename_forward_edge_leaves_inverse_back_edge_untouched() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);

        // Rename the `order` forward edge on OrderItem. Find-references lists the
        // inverse `Order.items` as a related site, but rename must not touch it (it
        // spells `items`, a different name).
        let oi_fid = snap.file_id_of(&root.join("order_item/model.bsl")).unwrap();
        let off = (snap.sources[oi_fid].1.find("order:").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(oi_fid, off, "parent")
            .expect("forward edge is renameable");
        let texts = rename_texts(&snap, &changes);
        assert!(
            texts.iter().all(|t| t == "order"),
            "only `order` sites rewritten, not the `items` back-edge: {texts:?}"
        );
        assert!(
            !texts.is_empty() && texts.iter().any(|t| t == "order"),
            "the declaration itself is rewritten: {texts:?}"
        );
    }


    #[test]
    fn rename_rejects_bad_target_and_non_symbol_cursor() {
        let ws = TempWorkspace::new("rename_reject");
        ws.write("based.toml", "");
        ws.write("org.bsl", "Org { name: text }\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("org.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // A non-identifier new name is refused (would produce unparseable source).
        let decl = (src.find("Org").unwrap() + 1) as u32;
        assert!(snap.rename_edits(fid, decl, "1bad").is_none());
        assert!(snap.rename_edits(fid, decl, "has space").is_none());

        // The cursor on a primitive keyword (`text`) is not a renameable symbol.
        let prim = (src.find("text").unwrap() + 1) as u32;
        assert!(snap.rename_edits(fid, prim, "blob").is_none());
        assert!(snap.prepare_rename_range(fid, prim).is_none());

        // prepareRename offers the identifier extent on the declaration.
        let r = snap
            .prepare_rename_range(fid, decl)
            .expect("Org is renameable");
        assert_eq!(snap.lines[fid].offset(r.start), src.find("Org").unwrap());
        assert_eq!(
            snap.lines[fid].offset(r.end),
            src.find("Org").unwrap() + "Org".len()
        );
    }


    /// (a) A callable param renames its declaration and every `$param` use in *that*
    /// callable's body — and only that callable's (params are callable-local).
    #[test]
    fn rename_param_rewrites_decl_and_local_uses_only() {
        let ws = TempWorkspace::new("rename_param");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget { qty: int? }\n\
             shape W from Widget { qty }\n\
             query find(min: int) -> W[] { list Widget where (qty > $min); }\n\
             query other(min: int) -> W[] { list Widget where (qty < $min); }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // Cursor on the `$min` use in `find` → rename to `floor`.
        let use_off = (src.find("qty > $min").unwrap() + "qty > $".len()) as u32;
        let changes = snap
            .rename_edits(fid, use_off, "floor")
            .expect("param is renameable");
        let texts = rename_texts(&snap, &changes);
        // The decl `min` + its one body use — both spelling `min`, nothing from `other`.
        assert_eq!(texts.len(), 2, "decl + local use only: {texts:?}");
        assert!(texts.iter().all(|t| t == "min"), "{texts:?}");

        // Applied: `find`'s param + use become `floor`; `other`'s `min` is untouched.
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        assert!(out.contains("query find(floor: int)"), "{out}");
        assert!(out.contains("qty > $floor"), "{out}");
        assert!(out.contains("query other(min: int)"), "{out}");
        assert!(out.contains("qty < $min"), "{out}");
    }


    /// (b) A `$ctx.<field>` bag field renames its `scope … = $ctx.field` binding and
    /// every callable use, leaving the scope *column* and same-named model columns alone.
    #[test]
    fn rename_ctx_field_rewrites_binding_and_uses() {
        let ws = TempWorkspace::new("rename_ctx");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "scope Tenant (org: Org = $ctx.org)\n\
             Org { name: text }\n\
             @scope Tenant\n\
             Widget { org: Org  name: text }\n\
             query mine() -> Widget[] scoped Tenant { list Widget where (org = $ctx.org); }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let src = &snap.sources[fid].1;

        // Cursor on the query's `$ctx.org` field segment → rename bag field to `tenant`.
        let off = (src.find("= $ctx.org)").unwrap() + "= $ctx.".len()) as u32;
        let changes = snap
            .rename_edits(fid, off, "tenant")
            .expect("ctx field is renameable");
        let texts = rename_texts(&snap, &changes);
        // The scope-term binding and the query use — two `org` segments, nothing else.
        assert_eq!(texts.len(), 2, "scope binding + query use: {texts:?}");
        assert!(texts.iter().all(|t| t == "org"), "{texts:?}");

        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        // Both `$ctx.org` become `$ctx.tenant`; the scope column `org:` and the model
        // field `org: Org` (and the `where` LHS `org`) keep their name.
        assert!(
            out.contains("scope Tenant (org: Org = $ctx.tenant)"),
            "{out}"
        );
        assert!(out.contains("where (org = $ctx.tenant)"), "{out}");
        assert!(out.contains("Widget { org: Org"), "{out}");
    }


    /// (c) A query name is a wire endpoint with no in-`.bsl` references, so rename
    /// rewrites just its declaration.
    #[test]
    fn rename_query_name_rewrites_declaration() {
        let ws = TempWorkspace::new("rename_query");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget { qty: int? }\n\
             shape W from Widget { qty }\n\
             query find() -> W[] { list Widget order (qty asc); }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("find()").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(fid, off, "list_widgets")
            .expect("query name is renameable");
        let texts = rename_texts(&snap, &changes);
        assert_eq!(texts, vec!["find".to_string()], "just the decl: {texts:?}");
    }


    /// (d) Renaming a field mapped to a *live* DB column also inserts `@was("old_col")`
    /// so the next generated migration renames the column (preserving data) — but only
    /// when the physical name actually changes and the column is captured in a snapshot.
    #[test]
    fn rename_field_inserts_was_for_live_column() {
        let ws = TempWorkspace::new("rename_was");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product {\n  name: text\n  barcode: text?\n}\n",
        );
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  \
             column name text not_null\n  column barcode text null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(
            !snap.diagnostics.iter().any(|d| d.code == "W0108"),
            "no drift expected: {:?}",
            snap.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("barcode:").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(fid, off, "code")
            .expect("field is renameable");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        // The rename + the data-preserving `@was` land on the field, and reparse clean.
        assert!(out.contains("code: text? @was(\"barcode\")"), "{out}");
        let reparsed = based_parser::parse_file(&out, FileId(0)).expect("applied source parses");
        let has_was = reparsed.decls.iter().any(|d| match d {
            Decl::Model(m) => m.members.iter().any(|mem| match mem {
                Member::Field(f) => {
                    f.name.node == "code"
                        && f.was.as_ref().map(|w| w.node.as_str()) == Some("barcode")
                }
                _ => false,
            }),
            _ => false,
        });
        assert!(has_was, "reparsed field carries @was(\"barcode\"): {out}");
    }


    /// A field with a `(column …)` override, an existing `@was`, or no captured
    /// migration snapshot gets no inserted `@was` (its physical name is decoupled,
    /// already named, or has no live column to preserve).
    #[test]
    fn rename_field_skips_was_when_not_data_preserving() {
        // Override decouples the physical name → renaming the field changes nothing in the DB.
        let ws = TempWorkspace::new("rename_was_skip");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Product {\n  name: text\n  barcode: text? (column \"upc\")\n}\n",
        );
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  \
             column name text not_null\n  column upc text null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("barcode:").unwrap() + 1) as u32;
        let out = apply_edits(
            &snap,
            fid,
            &snap.rename_edits(fid, off, "code").unwrap()[&file_uri(&snap, fid)],
        );
        assert!(!out.contains("@was"), "column override → no @was: {out}");

        // No captured migrations at all → nothing to preserve, no @was.
        let ws2 = TempWorkspace::new("rename_was_nomig");
        ws2.write("based.toml", "");
        ws2.write("schema.bsl", "Product {\n  barcode: text?\n}\n");
        let snap2 = compile_manifest(&ws2.root, &HashMap::new());
        let fid2 = snap2.file_id_of(&ws2.path("schema.bsl")).unwrap();
        let off2 = (snap2.sources[fid2].1.find("barcode:").unwrap() + 1) as u32;
        let out2 = apply_edits(
            &snap2,
            fid2,
            &snap2.rename_edits(fid2, off2, "code").unwrap()[&file_uri(&snap2, fid2)],
        );
        assert!(!out2.contains("@was"), "no migrations → no @was: {out2}");
    }


    /// Renaming a field already carrying `@was("orig")` keeps the *original* physical
    /// name as the was-source (the snapshot's column), so a rename chain still preserves
    /// data — it does not become `@was("<intermediate>")`.
    #[test]
    fn rename_field_keeps_original_was_across_a_chain() {
        let ws = TempWorkspace::new("rename_was_chain");
        ws.write("based.toml", "");
        // `barcode @was("upc")` is an uncaptured rename upc→barcode; the snapshot still
        // has `upc`. Renaming barcode→code must keep `@was("upc")`.
        ws.write(
            "schema.bsl",
            "Product {\n  name: text\n  barcode: text? @was(\"upc\")\n}\n",
        );
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  \
             column name text not_null\n  column upc text null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("barcode:").unwrap() + 1) as u32;
        let out = apply_edits(
            &snap,
            fid,
            &snap.rename_edits(fid, off, "code").unwrap()[&file_uri(&snap, fid)],
        );
        assert!(out.contains("code: text? @was(\"upc\")"), "{out}");
        assert!(
            !out.contains("@was(\"barcode\")"),
            "chain keeps orig: {out}"
        );
    }


    /// Renaming a model mapped to a live table inserts a leading `@was("old_table")`
    /// decorator, so the migration renames the table instead of drop+recreate.
    #[test]
    fn rename_model_inserts_was_for_live_table() {
        let ws = TempWorkspace::new("rename_model_was");
        ws.write("based.toml", "");
        ws.write("schema.bsl", "Product {\n  name: text\n}\n");
        ws.write(
            "migrations/0001_init/schema.snap",
            "snapshot v1 dialect=neutral\n\ntable product\n  column name text not_null\n",
        );
        ws.write("migrations/0001_init/up.mig", "# up\n");
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        let off = (snap.sources[fid].1.find("Product").unwrap() + 1) as u32;
        let out = apply_edits(
            &snap,
            fid,
            &snap.rename_edits(fid, off, "Item").unwrap()[&file_uri(&snap, fid)],
        );
        assert!(
            out.starts_with("@was(\"product\")\nItem {"),
            "leading @was decorator + renamed model: {out}"
        );
        let reparsed = based_parser::parse_file(&out, FileId(0)).expect("applied source parses");
        assert!(reparsed
            .decls
            .iter()
            .any(|d| matches!(d, Decl::Model(m) if m.name.node == "Item")));
    }


    #[test]
    fn rename_variant_rewrites_uses_and_leaves_same_named_variant_in_another_enum() {
        let (snap, fid) = enum_nav_snapshot();
        let src = &snap.sources[fid].1;
        // Rename `Status.pending` (the declaration) to `queued`.
        let off = (src.find("pending, paid, shipped").unwrap() + 1) as u32;
        let changes = snap
            .rename_edits(fid, off, "queued")
            .expect("variant is renameable");
        let out = apply_edits(&snap, fid, &changes[&file_uri(&snap, fid)]);
        // Status's variant + its `default` use become `queued`.
        assert!(
            out.contains("enum Status { queued, paid, shipped }"),
            "{out}"
        );
        assert!(out.contains("default queued"), "{out}");
        // Grade's same-named `pending` variant is untouched.
        assert!(out.contains("enum Grade { pending, top }"), "{out}");
    }

}
