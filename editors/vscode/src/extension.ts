// VS Code client for the Based DSL. Launches the `based-lsp` binary over stdio and
// wires it up as the language server for `.bsl` files, surfacing the diagnostics,
// inlay hints, and hover the server already emits (M5).
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const config = vscode.workspace.getConfiguration("basedls");
  const serverPath = config.get<string>("serverPath", "based-lsp");

  // The server communicates over stdio (tower-lsp). It takes no args; the client
  // sends the workspace root at `initialize`, and the server globs `**/*.bsl`.
  const serverOptions: ServerOptions = {
    run: { command: serverPath, transport: TransportKind.stdio },
    debug: { command: serverPath, transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    // Attach to every `.bsl` document.
    documentSelector: [{ scheme: "file", language: "bsl" }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.bsl"),
    },
    // The server publishes diagnostics unprompted; inlay hints + hover are pulled
    // via the capabilities it advertises at initialize. Nothing extra to enable
    // client-side beyond registering for the language — vscode-languageclient
    // negotiates the inlay-hint capability automatically when the server offers it.
  };

  client = new LanguageClient(
    "basedls",
    "Based DSL Language Server",
    serverOptions,
    clientOptions,
  );

  // start() also registers the client so it is disposed on deactivate.
  context.subscriptions.push(client);
  client.start().catch((err: unknown) => {
    void vscode.window.showErrorMessage(
      `Based DSL: failed to start language server "${serverPath}". ` +
        `Build it with \`cargo build -p based-lsp\` and set \`basedls.serverPath\` if it is not on PATH. (${String(err)})`,
    );
  });
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
