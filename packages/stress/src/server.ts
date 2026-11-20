// Starts, stops and kills a real `spool serve`, and measures it from outside.
// `packages/testing` finds the binary and starts the server.

import { existsSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { SpoolClient } from "spool-client";
import { makeHarness, makeScratchDirectory, repository } from "spool-testing";

export { repository };

// A debug build is several times slower, which is not what is measured
// here: the binary is the release build, unless `SPOOL_BIN` names one.
const harness = makeHarness("release");
export const { binary, spool } = harness;

export function scratch(name: string): string {
  return makeScratchDirectory(name, process.env.SPOOL_STRESS_DIR ?? join(tmpdir(), "spool-stress"));
}

export interface Running {
  url: string;
  /** A token of the server's, called `stress`, which `client` presents. */
  token: string;
  client: SpoolClient;
  pid: number;
  store: string;
  /** What the server wrote to its standard error so far. */
  complaints(): string;
  /** Megabytes of memory the server holds now. */
  memory(): number;
  /** Asks the server to stop and waits until it has. Resolves to its exit code. */
  stop(): Promise<number>;
  /** Stops it at once, as a crash or a lost machine would. */
  kill(): Promise<void>;
  /** Whether the process is still there. */
  alive(): boolean;
}

export interface ServeOptions {
  /** The project directory. It is built first. */
  project: string;
  /** The store's file. It is made if it is not there. */
  store: string;
  seed?: number;
  /** More arguments for `spool serve`. */
  args?: string[];
  build?: boolean;
}

/** Megabytes a process holds, as `ps` reports it. Zero once it is gone. */
export function memoryOf(pid: number): number {
  const result = Bun.spawnSync(["ps", "-o", "rss=", "-p", String(pid)], { stdout: "pipe", stderr: "ignore" });
  const kilobytes = Number(result.stdout.toString().trim());
  return Number.isFinite(kilobytes) ? Math.round(kilobytes / 1024) : 0;
}

/** Megabytes of the store's file and of the log DuckDB keeps beside it. */
export function storeSize(store: string): number {
  let bytes = 0;
  for (const path of [store, `${store}.wal`]) if (existsSync(path)) bytes += statSync(path).size;
  return Math.round(bytes / (1024 * 1024));
}

export async function serve({ project, store, seed, args = [], build = true }: ServeOptions): Promise<Running> {
  if (build) spool("build", project);
  const server = await harness.startServer({
    project,
    store,
    tasks: join(project, ".spool/tasks"),
    seed,
    args,
    errors: "kept",
  });

  // A token is shown once, so a server started again on the same store gets
  // a new one in place of the last.
  Bun.spawnSync([binary(), "token", "revoke", "stress", "--store", store], { stdout: "ignore", stderr: "ignore" });
  let token: string;
  try {
    token = spool("token", "add", "stress", "--store", store).trim();
  } catch (error) {
    // Nobody was given the server, so nobody would stop it.
    await server.kill();
    throw error;
  }

  return {
    url: server.url,
    token,
    client: new SpoolClient({ url: server.url, token }),
    pid: server.pid,
    store,
    complaints: server.complaints,
    memory: () => memoryOf(server.pid),
    alive: server.alive,
    stop: server.stop,
    kill: server.kill,
  };
}

/**
 * Checks a store no server has open, with the `verify` example of the
 * `spool-cli` crate. Resolves to what it printed, and throws if the store
 * does not verify.
 */
export function verify(store: string): string {
  // Built like the binary under test and from the same package, so that
  // nothing is compiled a second way only for this.
  const profile = binary().includes("/release/") ? ["--release"] : [];
  const result = Bun.spawnSync(
    ["cargo", "run", ...profile, "--quiet", "--locked", "-p", "spool-cli", "--example", "verify", "--", store],
    { cwd: repository, env: withoutTheTestsOwn(process.env), stdout: "pipe", stderr: "pipe" },
  );
  const printed = `${result.stdout}${result.stderr}`.trim();
  if (result.exitCode !== 0) throw new Error(printed);
  return printed;
}

/**
 * The environment without what Cargo tells a test about the package it is
 * in. Run from a test of the workspace, these tests have that in theirs,
 * and a build script that reads such a variable takes it for a change:
 * `ring` does, and Cargo then builds it again with all that is built on it,
 * the engine included, each time the stores are verified from a test.
 */
function withoutTheTestsOwn(env: NodeJS.ProcessEnv): Record<string, string> {
  const own = /^(CARGO_(MANIFEST_|PKG_|CRATE_|BIN_|PRIMARY_PACKAGE$|TARGET_TMPDIR$)|OUT_DIR$)/;
  const kept: Record<string, string> = {};
  for (const [name, value] of Object.entries(env)) {
    if (value !== undefined && !own.test(name)) kept[name] = value;
  }
  return kept;
}

/** Runs something and resolves to what it gave and the seconds it took. */
export async function timed<T>(work: () => Promise<T>): Promise<[T, number]> {
  const started = performance.now();
  const result = await work();
  return [result, (performance.now() - started) / 1000];
}
