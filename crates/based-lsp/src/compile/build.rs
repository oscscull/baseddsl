use super::*;

/// Walk up `file`'s ancestor directories to the nearest one holding a `based.toml`,
/// returning that directory — the manifest root that *owns* the file (the
/// rust-analyzer / tsserver project-marker model). `None` when no ancestor has a
/// manifest, i.e. the file rides under no project (single-file fallback).
pub fn find_manifest_root(file: &Path) -> Option<PathBuf> {
    // Canonicalize so the walk is over real ancestors; a not-yet-saved buffer
    // falls back to its raw path, whose parents are still meaningful.
    let start = canon(file);
    let mut dir = start.parent();
    while let Some(d) = dir {
        if d.join(based_manifest::MANIFEST_NAME).is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

// ---- Offline migration-drift diagnostic -------------------------------------

/// Compile the manifest project rooted at `root` (the dir holding `based.toml`),
/// with `overlays` (canonical path -> unsaved buffer text) taking precedence over
/// on-disk contents. Overlays for files outside this project are simply ignored.
pub fn compile_manifest(root: &Path, overlays: &HashMap<PathBuf, String>) -> Snapshot {
    match based_manifest::discover(root) {
        Ok(project) => {
            let dialect = based_codegen::Dialect::parse(&project.manifest.dialect);
            let fks = based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys);
            let pk_default = based_sema::PkStrategy::parse(&project.manifest.schema.id);
            let paths = project.files.into_iter().map(|f| f.path).collect();
            compile_paths(
                paths,
                overlays,
                Vec::new(),
                Some(root),
                Some(dialect),
                fks,
                pk_default,
            )
        }
        // Manifest present but unreadable/malformed: surface it as a project-level
        // diagnostic and still compile this project's open buffers so the editor
        // keeps giving single-file feedback.
        Err(diags) => {
            let mut ps: Vec<PathBuf> = overlays
                .keys()
                .filter(|p| find_manifest_root(p).as_deref() == Some(root))
                .cloned()
                .collect();
            ps.sort();
            compile_paths(
                ps,
                overlays,
                diags,
                Some(root),
                None,
                based_sema::ForeignKeys::None,
                based_sema::PkStrategy::Uuid,
            )
        }
    }
}

/// Compile a single `.bsl` file under no manifest in isolation (the fallback for a
/// file that belongs to no project — cross-file references cannot resolve here).
pub fn compile_loose(file: &Path, overlays: &HashMap<PathBuf, String>) -> Snapshot {
    compile_paths(
        vec![file.to_path_buf()],
        overlays,
        Vec::new(),
        None,
        None,
        based_sema::ForeignKeys::None,
        based_sema::PkStrategy::Uuid,
    )
}

/// Read + parse + check a fixed file set, preferring open buffers over disk, into a
/// snapshot. `project_diagnostics` carries any spanless project-level issues.
fn compile_paths(
    paths: Vec<PathBuf>,
    overlays: &HashMap<PathBuf, String>,
    project_diagnostics: Vec<Diagnostic>,
    migrations_root: Option<&Path>,
    dialect: Option<based_codegen::Dialect>,
    fks: based_sema::ForeignKeys,
    pk_default: based_sema::PkStrategy,
) -> Snapshot {
    // Read every file, preferring an open buffer over disk.
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(paths.len());
    for path in paths {
        let text = overlays
            .get(&canon(&path))
            .cloned()
            .unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default());
        sources.push((path, text));
    }

    // Parse each file; collect decls only if every file parsed clean (sema assumes
    // well-formed input, matching the CLI's precondition).
    let mut decls: Vec<Decl> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut parse_ok = true;
    for (i, (_, src)) in sources.iter().enumerate() {
        match based_parser::parse_file(src, FileId(i as u32)) {
            Ok(sf) => decls.extend(sf.decls),
            Err(diags) => {
                diagnostics.extend(diags);
                parse_ok = false;
            }
        }
    }

    let mut facts = Vec::new();
    let mut checked = None;
    if parse_ok {
        // Expand `...Shape` spreads on a working copy so sema/facts see a flat body; the
        // snapshot keeps the raw `decls` (spreads intact) for hover / go-to-definition.
        let mut expanded = decls.clone();
        diagnostics.extend(based_sema::expand_spreads(&mut expanded));
        let (mut schema, diags) = based_sema::check(&expanded);
        based_sema::resolve_pk_default(&mut schema, pk_default);
        diagnostics.extend(diags);
        // Target-specific checks need the manifest's compile target; a loose file
        // (no project) has none, so they are skipped there.
        if let Some(d) = dialect {
            diagnostics.extend(based_sema::check_target(&schema, d.name()));
            diagnostics.extend(based_codegen::ordered_nest_diagnostics(
                &schema, &expanded, d,
            ));
        }
        // FK-convention divergence checks (resolved project `foreign_keys`; a loose file
        // with no project uses the safe `None` default).
        diagnostics.extend(based_sema::check_foreign_keys(&schema, fks));
        facts = based_facts::facts(&schema, &expanded);
        // Offline migration-drift diagnostic: if the project has captured migrations and
        // the `.bsl` has structural changes not yet in one, flag them.
        if let Some(root) = migrations_root {
            diagnostics.extend(drift_diagnostics(root, &schema, &expanded, fks));
        }
        checked = Some(schema);
    }

    let lines = sources.iter().map(|(_, s)| LineIndex::new(s)).collect();
    Snapshot {
        sources,
        lines,
        facts,
        decls,
        diagnostics,
        project_diagnostics,
        schema: checked,
        migrations_root: migrations_root.map(Path::to_path_buf),
        dialect,
    }
}

/// Canonicalize for path comparison; fall back to the raw path if the file does
/// not resolve (e.g. an unsaved buffer whose path may not exist on disk yet).
pub fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::testsupport::*;

    #[test]
    fn compile_commerce_has_facts_and_no_diagnostics() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/commerce");
        let snap = compile_manifest(&root, &HashMap::new());
        assert!(nav_clean(&snap.diagnostics), "{:?}", snap.diagnostics);
        assert!(snap.project_diagnostics.is_empty());
        assert!(!snap.sources.is_empty());
        // The inferred inverse on `Order.items` is surfaced.
        assert!(
            snap.facts
                .iter()
                .any(|f| f.label.contains("via OrderItem.order")),
            "{:?}",
            snap.facts
        );
    }

    /// A file embedded in a host repo resolves to its own schema's `based.toml`,
    /// not the opened workspace root.
    #[test]
    fn find_manifest_root_walks_up_to_nearest_manifest() {
        let ws = TempWorkspace::new("walkup");
        ws.write("based.toml", "");
        ws.write("order/model.bsl", "Order { name: text }\n");
        let file = ws.path("order/model.bsl");

        let root = find_manifest_root(&file).expect("manifest above the file");
        assert_eq!(root, canon(&ws.root));

        // A file with no manifest anywhere above it has no project.
        let orphan = TempWorkspace::new("orphan");
        orphan.write("loose.bsl", "Order { name: text }\n");
        assert_eq!(find_manifest_root(&orphan.path("loose.bsl")), None);
    }

    /// The MySQL ordered-nest error must reach the editor: it needs the
    /// resolved compile target, so it only fires on the manifest-project path, and it
    /// must carry a span + an explanatory message the IDE can show inline.
    #[test]
    fn mysql_ordered_nest_surfaces_e0350() {
        let ws = TempWorkspace::new("mysql_ordered_nest");
        ws.write("based.toml", "dialect = \"mysql\"\n");
        ws.write(
            "schema.bsl",
            "@sort(created_at asc)\n\
             Comment { id: Id, body: text, created_at: timestamp, post: Post, @index(post) }\n\
             Post { id: Id, comments: Comment[] (Comment.post) }\n\
             shape PostDetail from Post { id, comments { id, body } }\n\
             query get_post(id) -> PostDetail;\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let d = snap
            .diagnostics
            .iter()
            .find(|d| d.code == "E0350")
            .unwrap_or_else(|| panic!("expected E0350, got {:?}", snap.diagnostics));
        assert!(d.span.is_some(), "E0350 must carry a span for the editor");
        assert!(d.message.contains("JSON_ARRAYAGG"), "{}", d.message);

        // The same schema on MariaDB has no such error.
        let ws2 = TempWorkspace::new("mariadb_ordered_nest");
        ws2.write("based.toml", "dialect = \"mariadb\"\n");
        ws2.write(
            "schema.bsl",
            "@sort(created_at asc)\n\
             Comment { id: Id, body: text, created_at: timestamp, post: Post, @index(post) }\n\
             Post { id: Id, comments: Comment[] (Comment.post) }\n\
             shape PostDetail from Post { id, comments { id, body } }\n\
             query get_post(id) -> PostDetail;\n",
        );
        let snap2 = compile_manifest(&ws2.root, &HashMap::new());
        assert!(
            !snap2.diagnostics.iter().any(|d| d.code == "E0350"),
            "MariaDB must permit an ordered nest: {:?}",
            snap2.diagnostics
        );
    }

    /// The misplaced field-`@sort` error must reach the editor with a span.
    #[test]
    fn misplaced_field_sort_surfaces_e0348() {
        let ws = TempWorkspace::new("misplaced_sort");
        ws.write("based.toml", "");
        ws.write(
            "schema.bsl",
            "Post { id: Id, created_at: timestamp @sort(created_at desc) }\n",
        );
        let snap = compile_manifest(&ws.root, &HashMap::new());
        let d = snap
            .diagnostics
            .iter()
            .find(|d| d.code == "E0348")
            .unwrap_or_else(|| panic!("expected E0348, got {:?}", snap.diagnostics));
        assert!(d.span.is_some(), "E0348 must carry a span for the editor");
    }

    /// Opening the repo root (no `based.toml` there) and editing a file whose model
    /// references a *sibling* file must resolve the whole manifest project — no
    /// spurious cross-file errors, unlike the single-file fallback.
    #[test]
    fn two_manifest_workspace_resolves_each_project_independently() {
        let ws = TempWorkspace::new("two_manifest");
        // Project A: a two-file schema with a cross-file relation.
        ws.write("a/based.toml", "");
        ws.write("a/org.bsl", "Org { name: text }\n");
        ws.write("a/user.bsl", "User {\n  org: Org\n  name: text\n}\n");
        // Project B: an independent, unrelated schema.
        ws.write("b/based.toml", "");
        ws.write("b/widget.bsl", "Widget { label: text }\n");

        // Each project compiles clean on its own manifest.
        let a = compile_manifest(&ws.path("a"), &HashMap::new());
        assert!(nav_clean(&a.diagnostics), "project A: {:?}", a.diagnostics);
        assert!(a.project_diagnostics.is_empty());
        let b = compile_manifest(&ws.path("b"), &HashMap::new());
        assert!(nav_clean(&b.diagnostics), "project B: {:?}", b.diagnostics);

        // The two projects are disjoint: B never sees A's models.
        assert!(b.sources.iter().all(|(p, _)| !p.ends_with("user.bsl")));

        // The cross-file reference (`User.org -> Org`) is what the manifest scope
        // buys: compiling `user.bsl` in isolation cannot see `Org`.
        let loose = compile_loose(&ws.path("a/user.bsl"), &HashMap::new());
        assert!(
            loose.diagnostics.iter().any(|d| d.code == "E0110"),
            "single-file fallback should not resolve the sibling model: {:?}",
            loose.diagnostics
        );
    }
}
