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
use compile::{compile, Snapshot};
use std::collections::HashMap;
use std::path::PathBuf;
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
    /// Workspace root (holds `based.toml`).
    root: Option<PathBuf>,
    /// Unsaved editor buffers, keyed by canonical path, overlaid on disk at compile.
    overlays: HashMap<PathBuf, String>,
    /// The latest compiled snapshot; requests are served from it.
    snapshot: Option<Snapshot>,
}

impl Backend {
    /// Recompile the project from disk + overlays, store the snapshot, and republish
    /// diagnostics for every file (empty vectors clear ones that were fixed).
    async fn refresh(&self) {
        let (root, overlays) = {
            let st = self.state.lock().unwrap();
            (st.root.clone(), st.overlays.clone())
        };
        let Some(root) = root else { return };
        let snapshot = compile(&root, &overlays);

        // Group span-carrying diagnostics by their file.
        let mut per_file: Vec<Vec<Diagnostic>> = vec![Vec::new(); snapshot.sources.len()];
        for d in &snapshot.diagnostics {
            if let Some(span) = d.span {
                let fid = span.file.0 as usize;
                if let (Some(bucket), Some(idx)) = (per_file.get_mut(fid), snapshot.lines.get(fid))
                {
                    bucket.push(to_lsp_diagnostic(d, idx));
                }
            }
        }
        // Collect the publishes while holding no lock across the awaits below.
        let mut publishes = Vec::new();
        for (i, (path, _)) in snapshot.sources.iter().enumerate() {
            if let Ok(uri) = Url::from_file_path(path) {
                publishes.push((uri, std::mem::take(&mut per_file[i])));
            }
        }
        let project_msgs: Vec<String> = snapshot
            .project_diagnostics
            .iter()
            .map(|d| format!("[{}] {}", d.code, d.message))
            .collect();

        self.state.lock().unwrap().snapshot = Some(snapshot);

        for (uri, diags) in publishes {
            self.client.publish_diagnostics(uri, diags, None).await;
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
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Prefer a workspace folder; fall back to the (deprecated) root_uri.
        let root = params
            .workspace_folders
            .and_then(|fs| fs.into_iter().next())
            .map(|f| f.uri)
            .or(params.root_uri)
            .and_then(|u| u.to_file_path().ok());
        self.state.lock().unwrap().root = root;

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
        let Some(snapshot) = &st.snapshot else {
            return Ok(None);
        };
        let Ok(path) = params.text_document.uri.to_file_path() else {
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
                FactKind::InferredIndex => idx.end_of_line(f.span.start as usize),
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

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let st = self.state.lock().unwrap();
        let Some(snapshot) = &st.snapshot else {
            return Ok(None);
        };
        let pos = params.text_document_position_params;
        let Ok(path) = pos.text_document.uri.to_file_path() else {
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

/// Map an internal diagnostic onto the LSP wire, resolving its span to a range.
fn to_lsp_diagnostic(d: &based_diagnostics::Diagnostic, idx: &compile::LineIndex) -> Diagnostic {
    let range = match d.span {
        Some(span) => Range::new(
            idx.position(span.start as usize),
            idx.position(span.end as usize),
        ),
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
