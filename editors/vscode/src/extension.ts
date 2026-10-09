// Own the extension's language-client lifecycle.
import * as vscode from "vscode";
import { LanguageClient } from "vscode-languageclient/node";
import { createClient } from "./language-client";
import { checkServer } from "./server-preflight";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const serverPath = vscode.workspace
    .getConfiguration("basedls")
    .get<string>("serverPath", "based-lsp");
  try {
    await checkServer(serverPath, context.extension.packageJSON.version as string);
    client = createClient(serverPath);
    context.subscriptions.push(client);
    await client.start();
  } catch (error: unknown) {
    void vscode.window.showErrorMessage(
      `Based DSL: ${error instanceof Error ? error.message : String(error)}`,
    );
    throw error;
  }
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
