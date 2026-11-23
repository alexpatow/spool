// What the client's test files share: a project of things, and the servers
// and directories a file's tests leave, which `cleanUp` takes away after them.

import { expect } from "bun:test";
import { rmSync } from "node:fs";

import { SpoolError } from "../src/index.ts";
import type { RunEntry, SpoolClient } from "../src/index.ts";
import { serve, temporaryDirectory, writeThings } from "./server.ts";
import type { Server } from "./server.ts";

const directories: string[] = [];
const servers: Server[] = [];

/** Stops the servers and removes the directories of a file's tests. Each file calls it after all of them. */
export async function cleanUp(): Promise<void> {
  for (const server of servers.splice(0)) await server.stop();
  for (const dir of directories.splice(0)) rmSync(dir, { recursive: true, force: true });
}

/** A directory that `cleanUp` removes. */
export function removeLater(dir: string): string {
  directories.push(dir);
  return dir;
}

/** Serves a project until `cleanUp`. */
export async function startServer(
  project: string,
  seed: number,
  environment: Record<string, string> = {},
): Promise<Server> {
  const server = await serve(project, seed, environment);
  servers.push(server);
  return server;
}

/** Serves a new project of things. */
export async function thingsServer(seed: number, environment: Record<string, string> = {}): Promise<Server> {
  const project = removeLater(temporaryDirectory());
  writeThings(project);
  return startServer(project, seed, environment);
}

export async function loadThings(client: SpoolClient, rows: Array<Record<string, string>>): Promise<RunEntry> {
  const run = await client.load({ source: "shop", entity_type: "thing", rows });
  expect(run.status).toBe("succeeded");
  return run;
}

export const twoAnvilsAndAHammer = [
  { id: "1", code: "anvil" },
  { id: "2", code: "anvil" },
  { id: "3", code: "hammer" },
];

/** The error a call that must fail throws. */
export async function failureOf(call: Promise<unknown>): Promise<SpoolError> {
  try {
    await call;
  } catch (error) {
    expect(error).toBeInstanceOf(SpoolError);
    return error as SpoolError;
  }
  throw new Error("the call succeeded");
}
