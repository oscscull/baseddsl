import { spawn, type ChildProcess } from "node:child_process";
import { createServer } from "node:http";
import { once } from "node:events";
import { listen, close } from "./listener.js";

export function command(binary: string, args: string[], cwd: string, env: NodeJS.ProcessEnv): ChildProcess {
  return spawn(binary, args, { cwd, env, stdio: ["ignore", "ignore", "pipe"] });
}

export async function run(binary: string, args: string[], cwd: string, env: NodeJS.ProcessEnv): Promise<void> {
  const child = command(binary, args, cwd, env);
  let diagnostics = "";
  child.stderr?.on("data", chunk => { diagnostics += chunk.toString(); });
  const [status] = await once(child, "exit");
  if (status !== 0) throw new Error(`based ${args.join(" ")} failed: ${diagnostics}`);
}

export async function startBased(binary: string, directory: string, env: NodeJS.ProcessEnv): Promise<{ process: ChildProcess; url: string }> {
  const reservation = createServer();
  const url = await listen(reservation);
  await close(reservation);
  const process = command(binary, ["serve", "--listen", new URL(url).host, "--guard-config", "guards.toml"], directory, env);
  let startupError: Error | undefined;
  process.on("error", error => { startupError = error; });
  let diagnostics = "";
  process.stderr?.on("data", chunk => { diagnostics += chunk.toString(); });
  try {
    for (let attempt = 0; attempt < 150; attempt++) {
      if (startupError) throw startupError;
      if (process.exitCode !== null) throw new Error(`based serve failed: ${diagnostics}`);
      try {
        const response = await fetch(`${url}/readyz`, { signal: AbortSignal.timeout(100) });
        if (response.ok) return { process, url };
      } catch { /* Socket may not yet be bound. */ }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    throw new Error("based serve readiness timed out");
  } catch (error) { process.kill(); throw error; }
}

export async function stop(process: ChildProcess): Promise<void> {
  if (process.exitCode !== null) return;
  process.kill("SIGTERM");
  const force = setTimeout(() => process.kill("SIGKILL"), 4000);
  try { await once(process, "exit"); } finally { clearTimeout(force); }
}
