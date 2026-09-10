use super::*;


impl Snapshot {
    /// Folding ranges for file `fid`: one region per top-level declaration whose body
    /// spans more than one line — a model, shape, scope, query, mutation, or filter.
    /// The range runs from the body's opening `{` (so the `Name {` header stays
    /// visible when collapsed) down to the closing delimiter; a brace-less decl (a
    /// multi-line scope / inline query / filter) folds from its first line. Extents
    /// come straight off the parsed decl spans — no text structure is re-derived.
    /// The file's canonical formatting, or `None` when there is nothing to apply —
    /// it doesn't parse (unformattable) or is already formatted. Delegates to the same
    /// `based_fmt` the CLI's `based fmt` runs, so the editor and CLI never diverge.
    pub fn format_document(&self, fid: usize) -> Option<String> {
        let src = &self.sources[fid].1;
        let formatted = based_fmt::format_source(src).ok()?;
        (formatted != *src).then_some(formatted)
    }

}


#[cfg(test)]
mod tests {
    use super::*;

    /// `format_document` is a no-op on the canonical commerce file, and rewrites an
    /// overlaid messy buffer into that same canonical layout.
    #[test]
    fn format_document_noop_then_reformats_overlay() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let model = root.join("order/model.bsl");

        // Canonical on disk: no edit to apply.
        let snap = compile_manifest(&root, &HashMap::new());
        let fid = snap.file_id_of(&model).unwrap();
        let canonical = snap.sources[fid].1.clone();
        assert_eq!(snap.format_document(fid), None);

        // Overlay a messy buffer for the same file: it formats back to the canonical text.
        let mut overlays = HashMap::new();
        let messy = "Order {\n  deleted_at:timestamp?\n     org: Org\n}\n";
        overlays.insert(std::fs::canonicalize(&model).unwrap(), messy.to_string());
        let snap = compile_manifest(&root, &overlays);
        let fid = snap.file_id_of(&model).unwrap();
        let formatted = snap.format_document(fid).expect("messy buffer reformats");
        assert_eq!(
            formatted,
            "Order {\n  deleted_at: timestamp?\n  org:        Org\n}\n"
        );
        assert_ne!(formatted, canonical); // the overlay replaced the on-disk model
    }

}
