use super::*;

/// A model's own field names as written in its decorators (`@key(…)`, `@sort(…)`,
/// `@soft_delete(…)`, `@created(…)`, `@updated(…)`) and `@index` members, each
/// rooted at the model itself.
pub(crate) fn model_attr_paths<'a>(m: &'a Model, out: &mut Vec<(&'a str, &'a [Ident])>) {
    let root = m.name.node.as_str();
    for d in &m.decorators {
        if !matches!(
            d.name.node.as_str(),
            "key" | "sort" | "soft_delete" | "created" | "updated"
        ) {
            continue;
        }
        for a in &d.args {
            match a {
                DecoArg::Ident(id) => out.push((root, std::slice::from_ref(id))),
                DecoArg::Path(p) => out.push((root, &p.segments)),
                DecoArg::Sort(t) => out.push((root, &t.path.segments)),
                DecoArg::Pred(_) | DecoArg::Lit(_) => {}
            }
        }
    }
    for mem in &m.members {
        if let Member::Index(i) = mem {
            for col in &i.columns {
                out.push((root, std::slice::from_ref(col)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    const SCHEMA: &str = "@key(code, term)\n\
        @sort(term desc)\n\
        @soft_delete(gone_at)\n\
        Course {\n  \
          code: text\n  \
          term: int\n  \
          title: text\n  \
          gone_at: timestamp?\n  \
          @index(title, term)\n\
        }\n";

    fn snapshot(tag: &str) -> (TempWorkspace, Snapshot, usize) {
        let ws = TempWorkspace::new(tag);
        ws.write("based.toml", "");
        ws.write("schema.bsl", SCHEMA);
        let snap = compile_manifest(&ws.root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
        (ws, snap, fid)
    }

    #[test]
    fn decorator_and_index_field_names_resolve_to_their_fields() {
        let (_ws, snap, fid) = snapshot("model_attr_goto");
        let src = &snap.sources[fid].1;
        for (use_site, decl) in [
            ("code, term)", "code: text"),
            ("term)", "term: int"),
            ("term desc", "term: int"),
            ("gone_at)", "gone_at: timestamp"),
            ("title, term)", "title: text"),
        ] {
            let at = src.find(use_site).unwrap() + 1;
            let def = snap
                .definition_at(fid, at as u32)
                .unwrap_or_else(|| panic!("{use_site} resolves"));
            assert_eq!(def.start as usize, src.find(decl).unwrap(), "{use_site}");
        }
        let hover = snap
            .hover_at(fid, (src.find("code, term)").unwrap() + 1) as u32)
            .expect("key field hover");
        assert!(hover.contains("code: text"), "{hover}");
    }

    #[test]
    fn renaming_a_field_rewrites_its_key_sort_and_index_mentions() {
        let (_ws, snap, fid) = snapshot("model_attr_rename");
        let src = &snap.sources[fid].1;
        let at = src.find("term: int").unwrap() + 1;
        let changes = snap
            .rename_edits(fid, at as u32, "season")
            .expect("renameable");
        let edits = &changes[&file_uri(&snap, fid)];
        let out = apply_edits(&snap, fid, edits);
        assert!(out.contains("@key(code, season)"), "{out}");
        assert!(out.contains("@sort(season desc)"), "{out}");
        assert!(out.contains("@index(title, season)"), "{out}");
        assert!(out.contains("season: int"), "{out}");
        assert!(!out.contains("term"), "{out}");
    }
}
