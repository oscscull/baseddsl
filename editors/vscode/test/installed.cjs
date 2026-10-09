// Exercise editor capabilities supplied by the installed VSIX, not source code.
const assert = require('node:assert/strict');
const vscode = require('vscode');

async function eventually(probe, description) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const value = await probe();
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for ${description}`);
}

exports.run = async function () {
  const extension = vscode.extensions.getExtension('based.based-vscode');
  assert(extension, 'VSIX was not installed');
  assert(!extension.extensionPath.includes('/editors/vscode'), 'loaded repository source instead of installed VSIX');
  await extension.activate();
  const uri = vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, 'schema/item.bsl');
  const document = await vscode.workspace.openTextDocument(uri);
  const editor = await vscode.window.showTextDocument(document);
  const reference = document.positionAt(document.getText().indexOf('Item[]'));
  const hover = await eventually(async () => {
    const result = await vscode.commands.executeCommand('vscode.executeHoverProvider', uri, reference);
    return result?.length && result;
  }, 'hover');
  assert(hover.flatMap(result => result.contents).some(content => content.value.includes('Item')));
  const completions = await vscode.commands.executeCommand('vscode.executeCompletionItemProvider', uri, reference);
  assert(completions.items.some(item => item.label === 'Item' || item.label.label === 'Item'));
  const rename = await vscode.commands.executeCommand('vscode.executeDocumentRenameProvider', uri, reference, 'Record');
  assert(rename.get(uri).length === 3, 'rename must cover declaration, return type, and query target');
  const typeStart = document.getText().indexOf('text');
  await editor.edit(edit => edit.replace(new vscode.Range(document.positionAt(typeStart), document.positionAt(typeStart + 4)), 'UnknownType'));
  await eventually(() => vscode.languages.getDiagnostics(uri).some(d => d.severity === vscode.DiagnosticSeverity.Error), 'unknown type diagnostic');
  console.log('Installed VSIX: diagnostics, hover, completion, rename passed');
};
