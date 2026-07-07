# Based DSL — VS Code extension

Language support for the Based DSL (`.bsl`). It registers the `bsl` language, gives
you minimal syntax highlighting + bracket/comment editing, and — the point — launches
the `based-lsp` language server so you get live **diagnostics**, **inlay hints** (inferred
inverses, join-key indexes, per-callable `$ctx` requirements, resolved query shapes), and
**hover** while you write `.bsl`.

The extension is a thin client. All the intelligence lives in `based-lsp` (see
`crates/based-lsp`), which speaks standard LSP over stdio.

## Prerequisites

1. **Build the language server** from the repo root:

   ```sh
   cargo build -p based-lsp
   ```

   This produces `target/debug/based-lsp` (or `target/release/based-lsp` with
   `--release`). Put it on your `PATH` as `based-lsp`, or point the extension at it
   with the `basedls.serverPath` setting (see below).

2. **Node.js + npm** (Node 18+; developed against Node 20).

## Build the extension

From `editors/vscode/`:

```sh
npm install       # install vscode-languageclient + build deps
npm run compile   # tsc -> ./out/extension.js
```

## Configure the server path

By default the extension runs `based-lsp` from your `PATH`. If the binary lives
elsewhere (e.g. you didn't install it), set the path in VS Code settings:

```jsonc
{
  // absolute path, or relative to the workspace root
  "basedls.serverPath": "/path/to/baseddsl/target/debug/based-lsp"
}
```

`basedls.trace.server` (`off` | `messages` | `verbose`) turns on LSP wire tracing in the
"Based DSL Language Server" output channel for debugging.

## Run it (development)

Open `editors/vscode/` in VS Code and press **F5** ("Run Extension"). A new
Extension Development Host window opens; open a folder containing `.bsl` files (for
example `spec/examples/commerce`) and you should see inlay hints and diagnostics.

## Package a `.vsix`

```sh
npm run package
# or, without a devDependency on vsce:
npx @vscode/vsce package
```

This produces `based-vscode-<version>.vsix`. Install it into VS Code with:

```sh
code --install-extension based-vscode-0.1.0.vsix
```

## What the server surfaces today

- **Diagnostics** — every parse/sema error and lint, inline.
- **Inlay hints** — inferred inverse pairings, join-key indexes, per-callable `$ctx`
  requirement bags, and each query's resolved verb/target/cardinality/pagination.
- **Hover** — the fuller "why" behind any derived fact under the cursor.
- **Go-to-definition** — jump from a model/shape reference to its declaration.
- **Document symbols** — the outline / breadcrumbs (⇧⌘O): models (fields nested),
  shapes, queries, mutations, filters.
- **Completion** — model names in a type annotation (after `:`) or return type
  (after `->`), a base model's fields after a resolvable `.`, decorators after `@`,
  and the keyword vocabulary otherwise.

## LSP capability audit (Track C4)

The baseline a general-purpose language extension is expected to provide, and where
this one stands. This is the gap set the remaining Track C4 iterations close.

| Capability | Status | Notes |
|------------|--------|-------|
| Diagnostics (`publishDiagnostics`) | **have** | parse/sema errors + lints, pushed on edit |
| Inlay hints (`inlayHint`) | **have** | engine-derived facts (principle 8) — not a standard-language feature, a DSL bonus |
| Hover (`hover`) | **have** | the "why" behind a derived fact |
| Go-to-definition (`definition`) | **have** | model/shape references → declaration (D43) |
| Document symbols (`documentSymbol`) | **have** | outline / breadcrumbs (D44) |
| Syntax highlighting (TextMate) | **have** | models vs. builtins; type-name coloring (D43) |
| Completion (`completion`) | **have** | model names in type position, fields after a resolvable `.`, keyword/decorator set (D45) |
| Workspace symbols (`workspaceSymbol`) | **missing** | jump to any model/callable by name across the project |
| Find references (`references`) | **missing** | reference-site index (superset of the go-to-def collector) |
| Rename (`rename`) | **missing** | the natural pair of find-references |
| Folding ranges (`foldingRange`) | **missing** | block folding — cheap, expected |
| Selection ranges (`selectionRange`) | **missing** | expand/shrink selection — cheap, expected |
| Code actions (`codeAction`) | **missing** | lint quick-fixes (e.g. `W0103` → add `@index`) — borderline, include only if cheap |
| Semantic tokens (`semanticTokens`) | **N/A** | coloring is done via TextMate; a semantic-token re-do is out of scope |
| Formatting (`formatting`) | **deferred** | no `based fmt` exists yet — out of scope for C4 |
| Signature help (`signatureHelp`) | **deferred** | exotic for a declarative DSL — out of scope for C4 |
| Call hierarchy (`callHierarchy`) | **deferred** | no call graph in a schema DSL — out of scope for C4 |
| Debugging (DAP) | **N/A** | nothing to execute/step in a schema — not applicable |

Static editing behaviour (bracket matching, auto-closing pairs, `#` comment toggling)
is handled by `language-configuration.json`, not the server.

The extension is a thin client: once the server advertises a capability at
`initialize`, `vscode-languageclient` negotiates it automatically — no client-side
change is needed to surface a newly served feature.
