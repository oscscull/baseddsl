// Check actionable startup failures without loading a VS Code host.
const assert = require('node:assert/strict');
const { requireServerVersion } = require('../out/server-version');
const { checkServer } = require('../out/server-preflight');
const { version } = require('../package.json');
async function main() {
  assert.throws(() => requireServerVersion('based-lsp 9.0.0 (abcdef)', version), /same release/);
  assert.throws(() => requireServerVersion('legacy server', version), /unrecognized/);
  await assert.rejects(checkServer('/missing/based-lsp', version), /basedls.serverPath/);
  console.log('Startup error contracts passed');
}
main().catch(error => { console.error(error); process.exitCode = 1; });
