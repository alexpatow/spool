// Starts, stops and kills a real `spool serve`, and measures it from outside.

import { existsSync, mkdirSync, mkdtempSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { SpoolClient } from "../../client/src/index.ts";

export const repository = resolve(import.meta.dir, "../../..");

/**
 * The binary under test: `SPOOL_BIN`, or the release build of this checkout.
 * A debug build is several times slower, which is not what is measured here.
 */
export function binary(): string {
  const path = process.env.SPOOL_BIN ?? join(repository, "target/release/spool");
  if (!existsSync(path)) {
    throw new Error(
      `there is no spool binary at ${path}: run \`cargo build --release -p spool-cli\`, or set SPOOL_BIN`,
    );
  }
  return path;
}

/** Runs a `spool` command that has to succeed and returns what it printed. */
export function spool(...args: string[]): string {
  const result = Bun.spawnSync([binary(), ...args], { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) throw new Error(`spool ${args.join(" ")} failed:\n${result.stdout}${result.stderr}`);
  return result.stdout.toString();
}

export function scratch(name: string): string {
  const root = process.env.SPOOL_STRESS_DIR ?? join(tmpdir(), "spool-stress");
  mkdirSync(root, { recursive: true });
  return mkdtempSync(join(root, `${name}-`));
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
  const child = Bun.spawn(
    [
      binary(),
      "serve",
      "--manifest",
      join(project, ".spool/manifest.json"),
      "--project",
      project,
      "--tasks",
      join(project, ".spool/tasks"),
      "--store",
      store,
      "--listen",
      "127.0.0.1:0",
      ...(seed === undefined ? [] : ["--seed", String(seed)]),
      ...args,
    ],
    { stdout: "pipe", stderr: "pipe" },
  );

  let complaints = "";
  void (async () => {
    const decoder = new TextDecoder();
    const errors = child.stderr.getReader();
    for (;;) {
      const { value, done } = await errors.read();
      if (done) return;
      complaints += decoder.decode(value, { stream: true });
    }
  })();

  // The server prints the address it listens on.
  const reader = child.stdout.getReader();
  const decoder = new TextDecoder();
  let printed = "";
  let url: string | undefined;
  while (url === undefined) {
    const { value, done } = await reader.read();
    if (done) {
      await child.exited;
      throw new Error(`the server stopped before it listened:\n${printed}${complaints}`);
    }
    printed += decoder.decode(value, { stream: true });
    url = /listening on (http:\/\/\S+)/.exec(printed)?.[1];
  }
  void (async () => {
    for (;;) if ((await reader.read()).done) return;
  })();

  // A token is shown once, so a server started again on the same store gets
  // a new one in place of the last.
  Bun.spawnSync([binary(), "token", "revoke", "stress", "--store", store], { stdout: "ignore", stderr: "ignore" });
  const token = spool("token", "add", "stress", "--store", store).trim();

  return {
    url,
    token,
    client: new SpoolClient({ url, token }),
    pid: child.pid,
    store,
    complaints: () => complaints,
    memory: () => memoryOf(child.pid),
    alive: () => child.exitCode === null && child.signalCode === null,
    async stop() {
      child.kill("SIGTERM");
      return await child.exited;
    },
    async kill() {
      child.kill("SIGKILL");
      await child.exited;
    },
  };
}

/**
 * Checks a store no server has open, with the `verify` example of the
 * `spool` crate. Resolves to what it printed, and throws if the store does
 * not verify.
 */
export function verify(store: string): string {
  // Built like the binary under test, so that nothing is compiled a second
  // way only for this.
  const profile = binary().includes("/release/") ? ["--release"] : [];
  const result = Bun.spawnSync(
    ["cargo", "run", ...profile, "--quiet", "--locked", "-p", "spool", "--example", "verify", "--", store],
    { cwd: repository, stdout: "pipe", stderr: "pipe" },
  );
  const printed = `${result.stdout}${result.stderr}`.trim();
  if (result.exitCode !== 0) throw new Error(printed);
  return printed;
}

/** Runs something and resolves to what it gave and the seconds it took. */
export async function timed<T>(work: () => Promise<T>): Promise<[T, number]> {
  const started = performance.now();
  const result = await work();
  return [result, (performance.now() - started) / 1000];
}
