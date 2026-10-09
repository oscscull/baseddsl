// Install the packaged extension into a temporary profile and launch its smoke harness.
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { downloadAndUnzipVSCode, resolveCliArgsFromVSCodeExecutablePath, runTests } = require('@vscode/test-electron');
const { execFileSync } = require('node:child_process');
const { version } = require('../package.json');

async function profile(root, server) {
  const workspace = path.join(root, 'project');
  await fs.mkdir(path.join(workspace, '.vscode'), { recursive: true });
  await fs.mkdir(path.join(workspace, 'schema'));
  await fs.writeFile(path.join(workspace, '.vscode/settings.json'), JSON.stringify({ 'basedls.serverPath': server }));
  await fs.writeFile(path.join(workspace, 'based.toml'), 'dialect = "sqlite"\nroot = "schema"\n');
  await fs.writeFile(path.join(workspace, 'schema/item.bsl'), 'Item {\n  id: Id,\n  name: text\n}\nquery items() -> Item[] { list Item; }\n');
  return workspace;
}

async function main() {
  if (!process.env.BASED_LSP) throw new Error('Set BASED_LSP to the matching prebuilt language server');
  const root = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), 'based-vsix-smoke-')));
  try {
    const executable = process.env.VSCODE_EXECUTABLE || await downloadAndUnzipVSCode('stable');
    const workspace = await profile(root, path.resolve(process.env.BASED_LSP));
    const dirs = ['--user-data-dir', path.join(root, 'profile'), '--extensions-dir', path.join(root, 'extensions')];
    const [cli, ...args] = resolveCliArgsFromVSCodeExecutablePath(executable, { reuseMachineInstall: true });
    execFileSync(cli, [...args, ...dirs, '--install-extension', path.resolve(`based-vscode-${version}.vsix`)], { stdio: 'inherit' });
    await runTests({
      vscodeExecutablePath: executable,
      extensionDevelopmentPath: path.join(__dirname, 'harness'),
      extensionTestsPath: path.join(__dirname, 'installed.cjs'),
      launchArgs: [workspace, ...dirs, '--skip-welcome', '--skip-release-notes', '--disable-workspace-trust', '--no-sandbox'],
    });
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
