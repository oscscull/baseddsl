import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { cp, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import * as fixtures from "./generated/fixtures.js";

const binary = process.env.BASED_BIN ? resolve(process.env.BASED_BIN) : "based";
const source = fileURLToPath(new URL("../", import.meta.url));
let directory: string;
let connection: fixtures.Connection;

before(async () => {
  directory = await mkdtemp(join(tmpdir(), "based-ts-fixtures-"));
  await cp(join(source, "schema"), join(directory, "schema"), { recursive: true });
  await cp(join(source, "based.toml"), join(directory, "based.toml"));
  const databaseUrl = join(directory, "test.db");
  const run = promisify(execFile);
  await run(binary, ["migrate", "gen"], { cwd: directory });
  await run(binary, ["migrate", "apply", "--database-url", databaseUrl], { cwd: directory });
  connection = await fixtures.connect({ databaseUrl, cwd: directory, binary });
});

after(async () => {
  await connection?.close();
  await rm(directory, { recursive: true, force: true });
});

test("placeholders fill required columns", async () => {
  const org = await fixtures.org().name("Acme").insert(connection);
  assert.equal(org.name, "Acme");
  assert.match(org.slug, /^slug-\d+$/);

  const user = await fixtures.user(org).insert(connection);
  assert.equal(user.org, org.id);
  assert.equal(user.role, "requester", "an enum takes its first variant");
  assert.equal(user.active, true, "an unset column keeps its default");
});

test("required relations are arguments; rows and ids both name a parent", async () => {
  const org = await fixtures.org().insert(connection);
  const requester = await fixtures.user(org).insert(connection);
  const agent = await fixtures.user(org.id).role("agent").insert(connection);
  const ticket = await fixtures
    .ticket(org, requester)
    .assignee(agent)
    .subject("Printer on fire")
    .insert(connection);
  assert.equal(ticket.requester, requester.id);
  assert.equal(ticket.assignee, agent.id);
  assert.equal(ticket.due, null, "an unset optional column is null");
  assert.equal(typeof ticket.created_at, "string", "@created still stamps");
});

test("the engine rejects what it cannot insert", async () => {
  await assert.rejects(
    connection.insert("Nope", {}, []),
    (error: unknown) => error instanceof fixtures.FixtureError && error.code === "unknown_fixture",
  );
});
