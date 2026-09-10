use super::*;
pub(crate) use std::collections::HashSet;

/// A snapshot is navigation-clean when it has no hard error other than the
/// terse-schema conveniences these navigation/hover/rename tests skip: the
/// index/id requirements, which are exercised in based-sema
/// and the code-action test, not here.
pub(crate) fn nav_clean(diags: &[Diagnostic]) -> bool {
    !diags.iter().any(|d| {
        d.severity == based_diagnostics::Severity::Error && d.code != "E0260" && d.code != "E0261"
    })
}

/// Rename rewrites every occurrence spelling the old name across files — a model's
/// declaration and its cross-file type references — but never the differently-named
/// inverse back-edge that only pairs through it.
pub(crate) fn rename_texts(snap: &Snapshot, changes: &HashMap<Url, Vec<TextEdit>>) -> Vec<String> {
    changes
        .iter()
        .flat_map(|(uri, edits)| {
            let fid = snap.file_id_of(&uri.to_file_path().unwrap()).unwrap();
            let src = snap.sources[fid].1.clone();
            let idx = &snap.lines[fid];
            edits.iter().map(move |e| {
                let s = idx.offset(e.range.start);
                let en = idx.offset(e.range.end);
                src[s..en].to_string()
            })
        })
        .collect()
}

/// Apply a file's rename edits to its source, returning the rewritten text.
/// Edits are non-overlapping (a zero-width `@was` insertion may share a start
/// offset with the name replacement); applying highest-offset-first, and for an
/// equal start the wider replacement before the empty insertion, lands the
/// inserted decorator before the renamed name.
pub(crate) fn apply_edits(snap: &Snapshot, fid: usize, edits: &[TextEdit]) -> String {
    let idx = &snap.lines[fid];
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            (
                idx.offset(e.range.start),
                idx.offset(e.range.end),
                e.new_text.as_str(),
            )
        })
        .collect();
    spans.sort_by_key(|(s, e, _)| (*s, *e));
    let mut out = snap.sources[fid].1.clone();
    for (s, e, t) in spans.into_iter().rev() {
        out.replace_range(s..e, t);
    }
    out
}

pub(crate) const ENUM_NAV_SCHEMA: &str = "\
enum Status { pending, paid, shipped }\n\
enum Grade { pending, top }\n\
Order { status: Status (default pending)  total: int }\n\
shape OrderRow from Order { status  total }\n\
query paid_orders() -> OrderRow[] { list Order where (status = paid) order (total); }\n\
query open_orders() -> OrderRow[] { list Order where (status in (paid, shipped)) order (total); }\n\
mutation mark(id: Id) -> OrderRow { update Order where (id = $id) { status = shipped } }\n";

pub(crate) fn enum_nav_snapshot() -> (Snapshot, usize) {
    let ws = TempWorkspace::new("enum_nav");
    ws.write("based.toml", "");
    ws.write("schema.bsl", ENUM_NAV_SCHEMA);
    let snap = compile_manifest(&ws.root, &HashMap::new());
    assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
    let fid = snap.file_id_of(&ws.path("schema.bsl")).unwrap();
    (snap, fid)
}

/// The shared fixture for signature-binding navigation: a bound edge
/// (`user -> author`), two `op col` bindings, and an unbound same-name param.
pub(crate) fn binding_snapshot(tag: &str) -> (TempWorkspace, Snapshot) {
    let ws = TempWorkspace::new(tag);
    ws.write("based.toml", "");
    ws.write(
        "schema.bsl",
        "User { name: text }\n\
         Post {\n  author: User\n  tags: json\n  created_at: timestamp\n}\n\
         query by_author(user -> author) -> Post[];\n\
         query search(tag: json has tags, since: timestamp > created_at) -> Post[];\n\
         query by_name(name) -> User[];\n",
    );
    let snap = compile_manifest(&ws.root, &HashMap::new());
    assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
    (ws, snap)
}

/// The URI of file `fid` in a snapshot — a test convenience for indexing rename edits.
pub(crate) fn file_uri(snap: &Snapshot, fid: usize) -> Url {
    Url::from_file_path(&snap.sources[fid].0).unwrap()
}

pub(crate) struct TempWorkspace {
    pub(crate) root: PathBuf,
}

impl TempWorkspace {
    pub(crate) fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("based-lsp-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    pub(crate) fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub(crate) fn write(&self, rel: &str, contents: &str) {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }
}

/// Reconstruct the emitted semantic tokens into `(source-text, type-name)` pairs —
/// the readable form for asserting what got highlighted, decoding the LSP
/// relative-position stream back to absolute spans.
pub(crate) fn decode_semantic(snap: &Snapshot, fid: usize) -> Vec<(String, String)> {
    let idx = &snap.lines[fid];
    let src = &snap.sources[fid].1;
    let (mut line, mut ch) = (0u32, 0u32);
    let mut out = Vec::new();
    for t in snap.semantic_tokens(fid) {
        if t.delta_line == 0 {
            ch += t.delta_start;
        } else {
            line += t.delta_line;
            ch = t.delta_start;
        }
        let start = idx.offset(Position::new(line, ch));
        let end = idx.offset(Position::new(line, ch + t.length));
        out.push((
            src[start..end].to_string(),
            sqltok::TOKEN_TYPES[t.token_type as usize].to_string(),
        ));
    }
    out
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
