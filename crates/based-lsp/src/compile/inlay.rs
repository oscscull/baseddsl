use super::*;


/// The start byte offset of a spanned model member (field / index). A raw
/// soft-override carries no span, so it never anchors the insert.
fn member_start(m: &Member) -> Option<u32> {
    match m {
        Member::Field(f) => Some(f.span.start),
        Member::Index(i) => Some(i.span.start),
        Member::Generated(g) => Some(g.span.start),
        Member::SoftOverride(_) => None,
    }
}

// ---- Hover renderers ("what", rust-analyzer baseline) -----------------------


impl Snapshot {
    /// The inlay hints for file `fid`: one per derived fact anchored in it, each at
    /// the end of its line. The inferred-inverse hint is a command-clickable label
    /// part linking to the forward edge it pairs through (`OrderItem.order`); the
    /// index and resolved-query facts are plain `tag label` strings. The scope and
    /// `$ctx` contracts surface on hover, so they carry no inlay. The caller filters
    /// by the requested viewport range.
    pub fn inlay_hints(&self, fid: usize) -> Vec<InlayHint> {
        let idx = &self.lines[fid];
        let mut hints = Vec::new();
        for f in &self.facts {
            if f.span.file.0 as usize != fid {
                continue;
            }
            let position = match f.kind {
                FactKind::InferredInverse | FactKind::ResolvedQuery => {
                    idx.end_of_line(f.span.start as usize)
                }
                FactKind::Scope | FactKind::CtxRequirement => continue,
            };
            // An inferred inverse links to the forward edge it pairs through, so the
            // `via Model.field` hint is command-clickable; other facts are plain text.
            let label = match (f.kind, f.nav) {
                (FactKind::InferredInverse, Some(nav)) => {
                    InlayHintLabel::LabelParts(vec![InlayHintLabelPart {
                        value: f.label.clone(),
                        location: self.nav_location(nav),
                        tooltip: None,
                        command: None,
                    }])
                }
                _ => InlayHintLabel::String(format!("{} {}", f.kind.tag(), f.label)),
            };
            hints.push(InlayHint {
                position,
                label,
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: Some(InlayHintTooltip::String(f.detail.clone())),
                padding_left: Some(true),
                padding_right: None,
                data: None,
            });
        }
        hints
    }


    /// The edit a quick-fix applies to insert `line` as the first member of model
    /// `model`'s body: `(file id, TextEdit)`. The line lands at the top of the body,
    /// matching the existing members' indentation (or two spaces on an empty body).
    /// Diagnostics that carry a `fix` name the model + line; the handler builds the
    /// edit here so the fix logic stays in one place.
    pub fn member_insert_edit(&self, model: &str, line: &str) -> Option<(usize, TextEdit)> {
        let m = self.decls.iter().find_map(|d| match d {
            Decl::Model(m) if m.name.node == model => Some(m),
            _ => None,
        })?;
        let fid = m.name.span.file.0 as usize;
        let src = &self.sources.get(fid)?.1;
        let idx = self.lines.get(fid)?;

        // Anchor at the first member's line, so the insert adopts its indentation and
        // sits at the top of the body. With no members, drop in right after the `{`.
        if let Some(first) = m.members.iter().filter_map(member_start).min() {
            let ls = line_start(src, first);
            let indent: String = src[ls..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let pos = idx.position(ls);
            let edit = TextEdit {
                range: Range::new(pos, pos),
                new_text: format!("{indent}{line}\n"),
            };
            return Some((fid, edit));
        }
        let brace = src[m.name.span.end as usize..].find('{')? + m.name.span.end as usize + 1;
        let pos = idx.position(brace);
        Some((
            fid,
            TextEdit {
                range: Range::new(pos, pos),
                new_text: format!("\n  {line}"),
            },
        ))
    }


    /// A cross-file `Location` for a span, resolving its `FileId` to the owning URI —
    /// the command-click target of an inlay label part (an inverse's forward edge).
    fn nav_location(&self, span: Span) -> Option<Location> {
        let fid = span.file.0 as usize;
        let (path, _) = self.sources.get(fid)?;
        let uri = Url::from_file_path(path).ok()?;
        Some(Location {
            uri,
            range: span_range(span, &self.lines[fid]),
        })
    }

}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    /// The inferred-inverse inlay is a *clickable* label part: `via OrderItem.order`
    /// whose location points at the `order` forward edge in order_item/model.bsl, so
    /// command-clicking the hint navigates to the edge it pairs through.
    #[test]
    fn inverse_inlay_is_a_clickable_label_part() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let model_fid = snap.file_id_of(&root.join("order/model.bsl")).unwrap();

        let hints = snap.inlay_hints(model_fid);
        // Find the inverse hint (the only label-parts hint) and inspect its part.
        let parts = hints
            .iter()
            .find_map(|h| match &h.label {
                InlayHintLabel::LabelParts(p) => Some(p),
                InlayHintLabel::String(_) => None,
            })
            .expect("an inverse hint rendered as label parts");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].value, "via OrderItem.order");

        // The part carries a location, and it points at `order` in order_item/model.bsl.
        let loc = parts[0].location.as_ref().expect("clickable location");
        assert!(
            loc.uri.path().ends_with("order_item/model.bsl"),
            "{}",
            loc.uri
        );
        let oi_fid = snap.file_id_of(&root.join("order_item/model.bsl")).unwrap();
        let oi_src = &snap.sources[oi_fid].1;
        let off = snap.lines[oi_fid].offset(loc.range.start);
        assert!(oi_src[off..].starts_with("order"), "lands on the edge name");

        // VS Code activates the label part by running go-to-def *at* its location
        // (LSP 3.17), so that must resolve — otherwise the click is inert even though
        // the link underlines. The forward edge's declaration resolves to itself.
        let def = snap
            .definition_at(oi_fid, off as u32)
            .expect("the click's go-to-def-at-location round-trips");
        assert!(oi_src[def.start as usize..].starts_with("order"));
    }


    /// The one-key quick-fix for a missing `id` and an unindexed query:
    /// each diagnostic carries a `fix` naming the model + the member line, and
    /// `member_insert_edit` drops that line at the top of the model's body, adopting
    /// the existing members' indentation.
    #[test]
    fn index_and_id_quickfixes_insert_the_missing_member() {
        let ws = TempWorkspace::new("quickfix");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Widget {\n  org: text\n  name: text\n}\n\
             shape W from Widget { name }\n\
             query by_org(org) -> W[];\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();

        // The missing-id case carries a `Widget` / `id: Id` fix.
        let no_id = snap
            .diagnostics
            .iter()
            .find(|d| d.code == "E0261")
            .and_then(|d| d.fix.as_ref())
            .expect("E0261 carries a fix");
        assert_eq!(
            (no_id.model.as_str(), no_id.line.as_str()),
            ("Widget", "id: Id")
        );
        let (efid, edit) = snap.member_insert_edit("Widget", "id: Id").expect("edit");
        assert_eq!(efid, fid);
        assert_eq!(edit.new_text, "  id: Id\n");
        // It lands at the top of the body, before the first member `org`.
        let org_line = snap.lines[fid].position(snap.sources[fid].1.find("  org: text").unwrap());
        assert_eq!(edit.range.start, org_line);

        // The unindexed-query case (scanning `org`) carries a `Widget` / `@index org` fix.
        let scan = snap
            .diagnostics
            .iter()
            .find(|d| d.code == "E0260")
            .and_then(|d| d.fix.as_ref())
            .expect("E0260 carries a fix");
        assert_eq!(
            (scan.model.as_str(), scan.line.as_str()),
            ("Widget", "@index org")
        );
        let (_, edit) = snap
            .member_insert_edit("Widget", "@index org")
            .expect("edit");
        assert_eq!(edit.new_text, "  @index org\n");
    }

}
