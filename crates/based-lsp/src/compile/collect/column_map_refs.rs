use super::*;

/// A model's composite-relation column-map key parts, each rooted at the relation's
/// target: `(column tenant_id = "…")` names the target's `tenant_id` key field.
pub(crate) fn column_map_paths<'a>(m: &'a Model, out: &mut Vec<(&'a str, &'a [Ident])>) {
    for mem in &m.members {
        let Member::Field(f) = mem else { continue };
        let BaseType::Model(target) = &f.ty.base else {
            continue;
        };
        for md in &f.modifiers {
            if let Modifier::ColumnMap(aliases) = md {
                for alias in aliases {
                    out.push((target.node.as_str(), std::slice::from_ref(&alias.part)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    const SCHEMA: &str = "@key(code)\n\
        Course { code: text  title: text }\n\
        @key(course, slot)\n\
        Enrollment { course: Course  slot: int  grade: int }\n\
        Session {\n  \
          enrollment: Enrollment (column slot = \"ses_slot\", course = \"ses_course\")\n  \
          note: text\n\
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
    fn column_map_key_parts_resolve_to_the_target_key_fields() {
        let (_ws, snap, fid) = snapshot("column_map_goto");
        let src = &snap.sources[fid].1;
        for (part, decl) in [("slot = ", "slot: int"), ("course = ", "course: Course")] {
            let at = src.find(part).unwrap() + 1;
            let def = snap
                .definition_at(fid, at as u32)
                .expect("key part resolves");
            assert_eq!(def.start as usize, src.find(decl).unwrap(), "{part}");
        }
        let hover = snap
            .hover_at(fid, (src.find("slot = ").unwrap() + 1) as u32)
            .expect("key part hover");
        assert!(hover.contains("slot: int"), "{hover}");
    }

    #[test]
    fn renaming_a_target_key_field_rewrites_its_column_map_part() {
        let (_ws, snap, fid) = snapshot("column_map_rename");
        let src = &snap.sources[fid].1;
        let at = src.find("slot = ").unwrap() + 1;
        let changes = snap
            .rename_edits(fid, at as u32, "seat")
            .expect("renameable");
        let edits = &changes[&file_uri(&snap, fid)];
        let out = apply_edits(&snap, fid, edits);
        assert!(out.contains("seat: int"), "{out}");
        assert!(out.contains("(column seat = \"ses_slot\""), "{out}");
    }
}
