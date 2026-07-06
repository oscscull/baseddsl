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

Go-to-definition, completion, and rename are not implemented yet (the server doesn't
serve them). See `PLAN.md` (M5 deferred / Track C).
