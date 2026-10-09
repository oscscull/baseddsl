// Execute the configured server's bounded identity probe without a shell.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { requireServerVersion } from "./server-version";
const execute = promisify(execFile);

export async function checkServer(command: string, version: string): Promise<void> {
  let identity: string;
  try {
    const result = await execute(command, ["--version"], {
      timeout: 5000, maxBuffer: 4096, windowsHide: true,
    });
    identity = result.stdout;
  } catch {
    throw new Error(
      `Cannot run "${command} --version". Install based-lsp ${version}, ` +
        `then set basedls.serverPath to its executable or put it on PATH.`,
    );
  }
  requireServerVersion(identity, version);
}
