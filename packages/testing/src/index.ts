// Finds the `spool` binary, runs its commands and starts real servers, for
// the tests of the packages and the scripts beside them.

import { cpSync, existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { SpoolClient } from "spool-client";
import type { Fetch, Person } from "spool-client";

export { readCsv } from "./csv.ts";

export const repository = resolve(import.meta.dir, "../../..");

/**
 * Which build of this checkout is the binary under test when `SPOOL_BIN`
 * names none. A debug build is several times slower than a release build,
 * which matters where the server is measured and not where it is only called.
 */
export type Build = "debug" | "release";

/** Makes a new, empty directory under `root`, whose name begins with `prefix`. */
export function makeScratchDirectory(prefix: string, root: string = tmpdir()): string {
  mkdirSync(root, { recursive: true });
  return mkdtempSync(join(root, `${prefix}-`));
}

/**
 * Copies a project's files into a new scratch directory, so that building it
 * leaves the original alone.
 */
export function copyProject(from: string, prefix: string): string {
  const to = makeScratchDirectory(prefix);
  for (const part of ["sources", "transforms", "entities", "extractors"]) {
    if (existsSync(join(from, part))) cpSync(join(from, part), join(to, part), { recursive: true });
  }
  return to;
}

export interface StartOptions {
  /** The project directory, which is already built. */
  project: string;
  /** The store's file. It is made if it is not there. */
  store: string;
  /** Where the server keeps the files of its tasks. */
  tasks: string;
  /** Makes the IDs the server mints reproducible. */
  seed?: number | undefined;
  /** More arguments for `spool serve`. */
  args?: string[];
  /** The server's environment: that of the tests, unless given. */
  environment?: Record<string, string | undefined>;
  /**
   * Where what the server writes to its standard error goes: `shown` with
   * the tests' own, or `kept` for `complaints()`.
   */
  errors?: "shown" | "kept";
  /** Whether to wait for the link that makes the first account, which a server nobody has an account on prints. */
  awaitSetupLink?: boolean;
}

/** A `spool serve` that listens. */
export interface ServerProcess {
  url: string;
  pid: number;
  /** The code of the link that makes the first account, when it was waited for. */
  setupCode: string | undefined;
  /** What the server wrote to its standard error so far, when that is kept. */
  complaints(): string;
  /** Whether the process is still there. */
  alive(): boolean;
  /** Asks the server to stop and waits until it has. Resolves to its exit code. */
  stop(): Promise<number>;
  /** Stops it at once, as a crash or a lost machine would. */
  kill(): Promise<void>;
}

/** A server of a project on a store of its own, with a token to call it with. */
export interface Server {
  url: string;
  /** A token of the server's, called `api`. */
  token: string;
  /** A client with that token: an agent, not a person. */
  client: SpoolClient;
  /** The code of the link the server printed, which makes the first account. */
  setupCode: string;
  stop(): Promise<void>;
}

export interface Harness {
  /** The binary under test: `SPOOL_BIN`, or the harness's build of this checkout. */
  binary(): string;
  /** Runs a `spool` command that has to succeed and returns what it printed. */
  spool(...args: string[]): string;
  /** Starts `spool serve` on a project that is built and resolves once it listens. */
  startServer(options: StartOptions): Promise<ServerProcess>;
  /**
   * Builds the project in `project` and serves it on a new store, on a port
   * the server picks. `seed` makes the IDs it mints reproducible, and
   * `environment` is set for the server.
   */
  serve(project: string, seed: number, environment?: Record<string, string>): Promise<Server>;
}

/** What makes a server a client of Google's. A test sets its own. */
const GOOGLE = ["SPOOL_GOOGLE_CLIENT_ID", "SPOOL_GOOGLE_CLIENT_SECRET", "SPOOL_GOOGLE_ISSUER", "SPOOL_PUBLIC_URL"];

/** A harness whose binary is `SPOOL_BIN`, or else the given build of this checkout. */
export function makeHarness(build: Build): Harness {
  function binary(): string {
    const path = process.env.SPOOL_BIN ?? join(repository, "target", build, "spool");
    if (!existsSync(path)) {
      const command = build === "release" ? "cargo build --release -p spool-cli" : "cargo build -p spool-cli";
      throw new Error(`there is no spool binary at ${path}: run \`${command}\`, or set SPOOL_BIN`);
    }
    return path;
  }

  function spool(...args: string[]): string {
    const result = Bun.spawnSync([binary(), ...args], { stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) throw new Error(`spool ${args.join(" ")} failed:\n${result.stdout}${result.stderr}`);
    return result.stdout.toString();
  }

  async function startServer({
    project,
    store,
    tasks,
    seed,
    args = [],
    environment,
    errors = "shown",
    awaitSetupLink = false,
  }: StartOptions): Promise<ServerProcess> {
    const child = Bun.spawn(
      [
        binary(),
        "serve",
        "--manifest",
        join(project, ".spool/manifest.json"),
        "--project",
        project,
        "--tasks",
        tasks,
        "--store",
        store,
        "--listen",
        "127.0.0.1:0",
        ...(seed === undefined ? [] : ["--seed", String(seed)]),
        ...args,
      ],
      {
        stdout: "pipe",
        stderr: errors === "kept" ? "pipe" : "inherit",
        ...(environment === undefined ? {} : { env: environment }),
      },
    );

    let complaints = "";
    const kept = child.stderr;
    if (kept !== undefined) {
      void (async () => {
        const decoder = new TextDecoder();
        const reader = kept.getReader();
        for (;;) {
          const { value, done } = await reader.read();
          if (done) return;
          complaints += decoder.decode(value, { stream: true });
        }
      })();
    }

    // The server prints the address it listens on, and then, when nobody
    // has an account yet, the link that makes the first one.
    const reader = child.stdout.getReader();
    const decoder = new TextDecoder();
    let printed = "";
    let url: string | undefined;
    let setupCode: string | undefined;
    while (url === undefined || (awaitSetupLink && setupCode === undefined)) {
      const { value, done } = await reader.read();
      if (done) {
        await child.exited;
        throw new Error(`the server stopped before it listened:\n${printed}${complaints}`);
      }
      printed += decoder.decode(value, { stream: true });
      url = /listening on (http:\/\/\S+)/.exec(printed)?.[1];
      setupCode = /\/setup#(\w+)\n/.exec(printed)?.[1];
    }
    // What it prints later is read and dropped, so that it never waits to be read.
    void (async () => {
      for (;;) if ((await reader.read()).done) return;
    })();

    return {
      url,
      pid: child.pid,
      setupCode,
      complaints: () => complaints,
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

  async function serve(project: string, seed: number, environment: Record<string, string> = {}): Promise<Server> {
    spool("build", project);
    const dir = makeScratchDirectory("spool-server");
    const store = join(dir, "store.duckdb");
    const server = await startServer({
      project,
      store,
      tasks: join(dir, "tasks"),
      seed,
      environment: {
        ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !GOOGLE.includes(name))),
        ...environment,
      },
      awaitSetupLink: true,
    });
    // Made on the server's machine, as an operator would for a pipeline.
    const token = spool("token", "add", "api", "--store", store).trim();

    return {
      url: server.url,
      token,
      client: new SpoolClient({ url: server.url, token }),
      setupCode: server.setupCode!,
      async stop() {
        await server.stop();
        rmSync(dir, { recursive: true, force: true });
      },
    };
  }

  return { binary, spool, startServer, serve };
}

/** Someone signed in, as their browser is: it keeps the session cookie. */
export interface Browser {
  person: Person;
  /** A `fetch` that sends the cookie and keeps the one it is given. */
  fetch: Fetch;
  /** A client that calls as this person. */
  client: SpoolClient;
}

export const PASSWORD = "a long enough one";

/** A `fetch` that keeps the session cookie between calls, as a browser does. */
export function cookies(): Fetch {
  let cookie: string | undefined;
  return async (input, init) => {
    const headers = new Headers(init.headers);
    if (cookie !== undefined) headers.set("Cookie", cookie);
    const response = await fetch(input, { ...init, headers });
    const set = response.headers.get("set-cookie");
    if (set !== null) cookie = set.split(";")[0];
    return response;
  };
}

/** Makes the server's first account, an admin, and signs in as it. */
export async function firstPerson(server: Server, name = "Ada"): Promise<Browser> {
  const browser = cookies();
  const client = new SpoolClient({ url: server.url, fetch: browser });
  const { person } = await client.setUp({
    code: server.setupCode,
    username: name.toLowerCase(),
    name,
    password: PASSWORD,
  });
  return { person: person!, fetch: browser, client };
}
