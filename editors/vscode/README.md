# Based DSL — VS Code extension

Licensed under [AGPL-3.0-only](LICENSE).

Language support for the Based DSL (`.bsl`). It registers the `bsl` language, gives
you minimal syntax highlighting + bracket/comment editing, and — the point — launches
the `based-lsp` language server so you get live **diagnostics**, **inlay hints** (inferred
inverses, join-key indexes, per-callable `$ctx` requirements, resolved query shapes), and
**hover** while you write `.bsl`.

The extension is a thin client. All the intelligence lives in `based-lsp` (see
`crates/based-lsp`), which speaks standard LSP over stdio.

## Install the matching release

Download `based-vscode-0.1.12.vsix` and the native archive for your platform
from the **same release**. Version `0.1.12` is currently a candidate; before an
owner-approved release, use artifacts from the same successful
[distribution workflow](../../docs/releasing.md) run. See the
[installation guide](../../docs/installation.md) for native platforms and checksums.

Extract the archive and install the VSIX:

```sh
code --install-extension based-vscode-0.1.12.vsix
```

Put the extracted `based-lsp` on PATH, or configure its absolute path:

```jsonc
{
  "basedls.serverPath": "/absolute/path/to/based-0.1.12/based-lsp"
}
```

On Windows, select `based-lsp.exe`. Open a folder containing your `.bsl` project.
Installed evaluation needs **no Rust or npm build**. The extension checks the
server version before opening a stdio session; a missing executable or mismatched
version reports how to install the matching server and configure its path.
Upgrade or roll back the VSIX and LSP together, then reload VS Code.

`basedls.trace.server` (`off` | `messages` | `verbose`) turns on LSP wire tracing
in the "Based DSL Language Server" output channel.

## Develop and package locally

Development requires Rust at the workspace minimum, Node 20+, and npm. From the
repository root, build `cargo build -p based-lsp`. Point `basedls.serverPath` at
that binary. From `editors/vscode/`:

```sh
npm ci
npm run compile
npm test
npm run package
```

The workspace and extension versions must match. Open `editors/vscode/` in VS Code
and press **F5** to launch an Extension Development Host.

The package smoke uses a separate temporary profile and installs the actual VSIX;
only its test harness is loaded in development mode. It checks diagnostics,
hover, model completion, and rename against the supplied prebuilt server:

```sh
BASED_LSP=/absolute/path/to/based-lsp npm run test:installed
```

It uses the official [VS Code test runner](https://code.visualstudio.com/api/working-with-extensions/testing-extension)
and downloads a stable VS Code test executable. To use an existing installation,
set `VSCODE_EXECUTABLE` to its executable (on macOS, `Contents/MacOS/Code`).
On headless Linux, use `xvfb-run -a npm run test:installed`. User settings and
installed extensions are isolated in a temporary directory that is removed after
verification.

## What the server surfaces today

- **Diagnostics** — every parse/sema error and lint, inline.
- **Inlay hints** — inferred inverse pairings (a `via <edge>` hint that command-clicks
  to the paired forward edge), join-key indexes, per-callable `$ctx` requirement bags,
  and each query's resolved verb/target/cardinality/pagination.
- **Hover** — the declaration of the symbol under the cursor (a field's `name: Type`, a
  model/shape/scope/callable signature), plus the fuller "why" behind any derived fact.
- **Go-to-definition** — jump from a model/shape/scope reference, a `filter(...)` call,
  *or a field-reference path* (`placed_by`, `placed_by.name`, a `where`/`order`/write-assign
  column) to its declaration, walking through relations from the statically-known root.
- **Find references** — every use of the symbol under the cursor: a model/shape/scope's
  references, a filter's call sites, a field's uses across shapes/queries/mutations, and —
  for a forward relation edge — the inverse `Model[]` that pairs through it (the back-follow).
- **Rename** (with a prepare step) — rewrites the symbol's declaration and every reference
  spelling its name across files; the inverse back-edge (a differently-named field that only
  pairs through it) is *not* renamed, unlike the find-references listing.
- **Document symbols** — the outline / breadcrumbs (⇧⌘O): models (fields nested),
  shapes, queries, mutations, filters.
- **Workspace symbols** — jump to any named declaration by name across the whole
  project (⌘T): models and their fields, shapes, scopes, queries, mutations, filters,
  fuzzy-filtered by the typed query.
- **Completion** — model names in a type annotation (after `:`) or return type
  (after `->`), a base model's fields after a resolvable `.`, decorators after `@`,
  and the keyword vocabulary otherwise.
- **Folding** — each multi-line declaration body (model, shape, scope, query,
  mutation, filter) collapses from its `{` header line to its closing brace.
- **Selection ranges** — expand/shrink selection walks the AST outward: the
  identifier token → its field declaration → the enclosing declaration → the file.

## LSP capability audit (Track C4)

The baseline a general-purpose language extension is expected to provide, and where
this one stands. This is the gap set the remaining Track C4 iterations close.

| Capability | Status | Notes |
|------------|--------|-------|
| Diagnostics (`publishDiagnostics`) | **have** | parse/sema errors + lints, pushed on edit |
| Inlay hints (`inlayHint`) | **have** | engine-derived facts (principle 8) — not a standard-language feature, a DSL bonus |
| Hover (`hover`) | **have** | the symbol's declaration ("what": field `name: Type`, model/shape/scope/callable signature) + the derived-fact "why" (C4a) |
| Go-to-definition (`definition`) | **have** | model/shape/scope refs, filter calls, field-reference paths (shape/`where`/`order`/write columns, walked through relations) → declaration (D43, C4a) |
| Find references (`references`) | **have** | every use of the symbol under the cursor — model/shape/scope refs, filter calls, field uses across bodies, and a forward edge's inverse back-edge (D52) |
| Document symbols (`documentSymbol`) | **have** | outline / breadcrumbs (D44) |
| Syntax highlighting (TextMate) | **have** | models vs. builtins; type-name coloring (D43) |
| Completion (`completion`) | **have** | model names in type position, fields after a resolvable `.`, keyword/decorator set (D45) |
| Workspace symbols (`workspaceSymbol`) | **have** | ⌘T — every named decl (models + fields, shapes, scopes, queries, mutations, filters) across the whole project, fuzzy-filtered (D54) |
| Rename (`rename`) + prepare (`prepareRename`) | **have** | rewrites every occurrence spelling the old name across files (reuses the D52 `references_at` index); leaves the differently-named inverse back-edge untouched (D53) |
| Folding ranges (`foldingRange`) | **have** | fold each multi-line declaration body — model/shape/scope/query/mutation/filter — from its `{` header to the close, off the parsed decl spans (D68) |
| Selection ranges (`selectionRange`) | **have** | expand/shrink selection through the AST: token → field → enclosing declaration → file (D68) |
| Code actions (`codeAction`) | **out of scope** | lint quick-fixes: `W0103` anchors on the query (often a different file than the model needing the `@index`) and carries no target-model span, so a correct fix needs new lint→diagnostic plumbing — not cheap; revisit with `based fmt` (D68) |
| Semantic tokens (`semanticTokens`) | **N/A** | coloring is done via TextMate; a semantic-token re-do is out of scope |
| Formatting (`formatting`) | **have** | whole-document formatting, delegating to the canonical `based fmt` (D78) |
| Signature help (`signatureHelp`) | **deferred** | exotic for a declarative DSL — out of scope for C4 |
| Call hierarchy (`callHierarchy`) | **deferred** | no call graph in a schema DSL — out of scope for C4 |
| Debugging (DAP) | **N/A** | nothing to execute/step in a schema — not applicable |

Static editing behaviour (bracket matching, auto-closing pairs, `#` comment toggling)
is handled by `language-configuration.json`, not the server.

The extension is a thin client: once the server advertises a capability at
`initialize`, `vscode-languageclient` negotiates it automatically — no client-side
change is needed to surface a newly served feature.
