// Configure the stdio transport and BSL document synchronization.
import * as vscode from "vscode";
import { LanguageClient, TransportKind } from "vscode-languageclient/node";

export function createClient(serverPath: string): LanguageClient {
  const transport = { command: serverPath, transport: TransportKind.stdio };
  return new LanguageClient(
    "basedls", "Based DSL Language Server",
    { run: transport, debug: transport },
    {
      documentSelector: [{ scheme: "file", language: "bsl" }],
      synchronize: {
        fileEvents: vscode.workspace.createFileSystemWatcher("**/*.bsl"),
      },
    },
  );
}
