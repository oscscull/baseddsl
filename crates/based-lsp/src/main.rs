//! based-lsp — the editor server for the `based` DSL.
//!
//! Its job is principle 8: *show, don't write* the facts the engine derives. On
//! every edit it recompiles the project (the same discover -> parse -> check front
//! end the CLI runs) and surfaces:
//!   * **diagnostics** — every parse/sema error + lint, inline;
//!   * **inlay hints** — the inferred inverse pairings and join-key indexes, shown
//!     next to the declarations they belong to but never written into source;
//!   * **hover** — the fuller "why" behind each derived fact.
//!
//! The derivation itself lives in `based-facts` (pure, golden-tested); this crate
//! is the transport that maps those facts onto the LSP wire.

mod compile;

use based_diagnostics::Severity;
use based_facts::FactKind;
use compile::{canon, compile_loose, compile_manifest, find_manifest_root, Snapshot};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

struct Backend {
    client: Client,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Unsaved editor buffers, keyed by canonical path, overlaid on disk at compile.
    overlays: HashMap<PathBuf, String>,
    /// One compiled snapshot per open project (nearest-manifest dir, or a lone file
    /// under no manifest); requests route to the snapshot owning the requested file.
    snapshots: HashMap<ProjectKey, Snapshot>,
    /// URIs we last published diagnostics to, so a project dropping out of the open
    /// set has its files cleared rather than left with stale squiggles.
    published: Vec<Url>,
}

/// The project an open file belongs to. Every file resolves to exactly one: the
/// nearest ancestor `based.toml` if there is one, else the file itself.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum ProjectKey {
    /// A manifest project, keyed by the dir holding `based.toml`.
    Manifest(PathBuf),
    /// A single `.bsl` file under no manifest (single-file fallback).
    Loose(PathBuf),
}

/// Resolve a file to its owning project (walk up to the nearest `based.toml`).
fn project_key(path: &Path) -> ProjectKey {
    match find_manifest_root(path) {
        Some(root) => ProjectKey::Manifest(root),
        None => ProjectKey::Loose(canon(path)),
    }
}

impl Backend {
    /// Recompile every open project from disk + overlays, store the snapshots, and
    /// republish diagnostics. Each open buffer belongs to exactly one project (its
    /// nearest manifest, or itself); we compile one snapshot per distinct project so
    /// cross-file references resolve and embedded schemas stay independent. A file is
    /// published from the snapshot of its *owning* project only, so nested manifests
    /// never double-publish; files dropped since last time are cleared.
    async fn refresh(&self) {
        let overlays = self.state.lock().unwrap().overlays.clone();

        let keys: HashSet<ProjectKey> = overlays.keys().map(|p| project_key(p)).collect();
        let mut snapshots: HashMap<ProjectKey, Snapshot> = HashMap::new();
        for key in keys {
            let snap = match &key {
                ProjectKey::Manifest(root) => compile_manifest(root, &overlays),
                ProjectKey::Loose(file) => compile_loose(file, &overlays),
            };
            snapshots.insert(key, snap);
        }

        // Collect the publishes (and project-level messages) while holding no lock
        // across the awaits below.
        let mut publishes: Vec<(Url, Vec<Diagnostic>)> = Vec::new();
        let mut project_msgs: Vec<String> = Vec::new();
        for (key, snapshot) in &snapshots {
            // Group span-carrying diagnostics by their file.
            let mut per_file: Vec<Vec<Diagnostic>> = vec![Vec::new(); snapshot.sources.len()];
            for d in &snapshot.diagnostics {
                if let Some(span) = d.span {
                    let fid = span.file.0 as usize;
                    if let (Some(bucket), Some(idx)) =
                        (per_file.get_mut(fid), snapshot.lines.get(fid))
                    {
                        bucket.push(to_lsp_diagnostic(d, idx));
                    }
                }
            }
            for (i, (path, _)) in snapshot.sources.iter().enumerate() {
                // Publish a file only from its owning project (a nested manifest's
                // file also appears in an outer project's glob — the nearest owns it).
                if project_key(path) != *key {
                    continue;
                }
                if let Ok(uri) = Url::from_file_path(path) {
                    publishes.push((uri, std::mem::take(&mut per_file[i])));
                }
            }
            for d in &snapshot.project_diagnostics {
                project_msgs.push(format!("[{}] {}", d.code, d.message));
            }
        }

        // Files no longer under any open project get an explicit clear.
        let now: HashSet<Url> = publishes.iter().map(|(u, _)| u.clone()).collect();
        let stale: Vec<Url> = {
            let mut st = self.state.lock().unwrap();
            let stale = st
                .published
                .iter()
                .filter(|u| !now.contains(u))
                .cloned()
                .collect();
            st.snapshots = snapshots;
            st.published = now.into_iter().collect();
            stale
        };

        for (uri, diags) in publishes {
            self.client.publish_diagnostics(uri, diags, None).await;
        }
        for uri in stale {
            self.client.publish_diagnostics(uri, Vec::new(), None).await;
        }
        for msg in project_msgs {
            self.client.show_message(MessageType::ERROR, msg).await;
        }
    }

    /// Record (or clear) an open buffer's text, then recompile.
    async fn set_overlay(&self, uri: &Url, text: String) {
        if let Ok(path) = uri.to_file_path() {
            let key = std::fs::canonicalize(&path).unwrap_or(path);
            self.state.lock().unwrap().overlays.insert(key, text);
        }
        self.refresh().await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        // No single workspace root: each open file resolves to its own project by
        // walking up to the nearest `based.toml` (see `project_key`), so a `.bsl`
        // schema embedded anywhere in the opened folder is found regardless of where
        // the folder is rooted.
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "based-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                inlay_hint_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.refresh().await;
        self.client
            .log_message(MessageType::INFO, "based-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.set_overlay(&params.text_document.uri, params.text_document.text)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // FULL sync: the last change contains the whole document.
        if let Some(change) = params.content_changes.into_iter().last() {
            self.set_overlay(&params.text_document.uri, change.text)
                .await;
        }
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        self.refresh().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // Drop the overlay so the file reverts to its on-disk contents.
        if let Ok(path) = params.text_document.uri.to_file_path() {
            let key = std::fs::canonicalize(&path).unwrap_or(path);
            self.state.lock().unwrap().overlays.remove(&key);
        }
        self.refresh().await;
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let st = self.state.lock().unwrap();
        let Ok(path) = params.text_document.uri.to_file_path() else {
            return Ok(None);
        };
        let Some(snapshot) = st.snapshots.get(&project_key(&path)) else {
            return Ok(None);
        };
        let Some(fid) = snapshot.file_id_of(&path) else {
            return Ok(None);
        };
        let idx = &snapshot.lines[fid];

        let mut hints = Vec::new();
        for f in &snapshot.facts {
            if f.span.file.0 as usize != fid {
                continue;
            }
            // An inverse hint sits after the field it annotates; an index hint (whose
            // span is the whole model) reads best at the end of the model's header.
            let position = match f.kind {
                FactKind::InferredInverse => idx.position(f.span.end as usize),
                // Model/callable-wide facts read best at the end of the header line.
                FactKind::InferredIndex | FactKind::CtxRequirement | FactKind::ResolvedQuery => {
                    idx.end_of_line(f.span.start as usize)
                }
            };
            if position < params.range.start || position > params.range.end {
                continue;
            }
            hints.push(InlayHint {
                position,
                label: InlayHintLabel::String(format!("{} {}", f.kind.tag(), f.label)),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: Some(InlayHintTooltip::String(f.detail.clone())),
                padding_left: Some(true),
                padding_right: None,
                data: None,
            });
        }
        Ok(Some(hints))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let st = self.state.lock().unwrap();
        let pos = params.text_document_position_params;
        let Ok(path) = pos.text_document.uri.to_file_path() else {
            return Ok(None);
        };
        // Route to the snapshot owning the requested file (its nearest manifest),
        // exactly as hover/inlay do, so cross-file references resolve.
        let Some(snapshot) = st.snapshots.get(&project_key(&path)) else {
            return Ok(None);
        };
        let Some(fid) = snapshot.file_id_of(&path) else {
            return Ok(None);
        };
        let offset = snapshot.lines[fid].offset(pos.position) as u32;

        // Resolve the reference under the cursor to its declaration's name span,
        // then point the editor at that span in whichever file declares it.
        let Some(def) = snapshot.definition_at(fid, offset) else {
            return Ok(None);
        };
        let def_fid = def.file.0 as usize;
        let Ok(uri) = Url::from_file_path(&snapshot.sources[def_fid].0) else {
            return Ok(None);
        };
        let range = span_to_range(def, &snapshot.lines[def_fid]);
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri,
            range,
        })))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let st = self.state.lock().unwrap();
        let Ok(path) = params.text_document.uri.to_file_path() else {
            return Ok(None);
        };
        // Route to the snapshot owning the file (its nearest manifest), like the
        // other position-based requests, then read its parsed decls for this file.
        let Some(snapshot) = st.snapshots.get(&project_key(&path)) else {
            return Ok(None);
        };
        let Some(fid) = snapshot.file_id_of(&path) else {
            return Ok(None);
        };
        Ok(Some(DocumentSymbolResponse::Nested(
            snapshot.document_symbols(fid),
        )))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let st = self.state.lock().unwrap();
        let pos = params.text_document_position_params;
        let Ok(path) = pos.text_document.uri.to_file_path() else {
            return Ok(None);
        };
        let Some(snapshot) = st.snapshots.get(&project_key(&path)) else {
            return Ok(None);
        };
        let Some(fid) = snapshot.file_id_of(&path) else {
            return Ok(None);
        };
        let idx = &snapshot.lines[fid];
        let offset = idx.offset(pos.position) as u32;

        // Any fact whose anchor span covers the cursor contributes its "why".
        let details: Vec<String> = snapshot
            .facts
            .iter()
            .filter(|f| {
                f.span.file.0 as usize == fid && f.span.start <= offset && offset < f.span.end
            })
            .map(|f| format!("**{}** — {}", f.kind.tag(), f.detail))
            .collect();
        if details.is_empty() {
            return Ok(None);
        }
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: details.join("\n\n"),
            }),
            range: None,
        }))
    }
}

/// Map a source `Span`'s byte range onto an LSP `Range` via the owning file's index.
fn span_to_range(span: based_ast::Span, idx: &compile::LineIndex) -> Range {
    Range::new(
        idx.position(span.start as usize),
        idx.position(span.end as usize),
    )
}

/// Map an internal diagnostic onto the LSP wire, resolving its span to a range.
fn to_lsp_diagnostic(d: &based_diagnostics::Diagnostic, idx: &compile::LineIndex) -> Diagnostic {
    let range = match d.span {
        Some(span) => span_to_range(span, idx),
        None => Range::default(),
    };
    let severity = Some(match d.severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    });
    let mut message = d.message.clone();
    for note in &d.notes {
        message.push_str("\nnote: ");
        message.push_str(note);
    }
    Diagnostic {
        range,
        severity,
        code: Some(NumberOrString::String(d.code.to_string())),
        source: Some("based".to_string()),
        message,
        ..Default::default()
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend {
        client,
        state: Mutex::new(State::default()),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
