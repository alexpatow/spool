// Starts the real `spool` binary for the tests to call.

import { cpSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { SpoolClient } from "../src/index.ts";
import type { Fetch, Person } from "../src/index.ts";

const here = dirname(fileURLToPath(import.meta.url));
export const repository = resolve(here, "../../..");
export const companies = join(repository, "benchmarks/identity/companies-small");
export const TOKEN = "client-test-token";

/** The binary under test: `SPOOL_BIN`, or the debug build of this checkout. */
export function binary(): string {
  const path = process.env.SPOOL_BIN ?? join(repository, "target/debug/spool");
  if (!existsSync(path)) {
    throw new Error(`there is no spool binary at ${path}: run \`cargo build -p spool-cli\`, or set SPOOL_BIN`);
  }
  return path;
}

/** Runs a command that has to succeed and returns what it printed. */
export function spool(...args: string[]): string {
  const result = Bun.spawnSync([binary(), ...args], { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) {
    throw new Error(`spool ${args.join(" ")} failed:\n${result.stdout}${result.stderr}`);
  }
  return result.stdout.toString();
}

export function temporaryDirectory(): string {
  return mkdtempSync(join(tmpdir(), "spool-client-"));
}

/**
 * A project with one entity type, `thing`: things with the same code match.
 * Rows come from `shop` and `bulk`, and `notes` is a source of documents.
 */
export function writeThings(dir: string): void {
  const rows = "key: id\ncolumns: [id, code, weight]\n";
  const files: Record<string, string> = {
    "sources/shop.yaml": rows,
    "sources/bulk.yaml": rows,
    "sources/notes.yaml": "columns: [code]\n",
    "entities/thing/entity.yaml":
      "label: code\nfeatures: [code]\nmapping:\n" +
      ["shop", "bulk", "notes"].map((source) => `  ${source}:\n    code: code\n`).join(""),
    "entities/thing/rules/block/same_code.sql": "l.code = r.code\n",
    "entities/thing/rules/match/same_code.sql": "l.code = r.code\n",
  };
  for (const [file, contents] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, file)), { recursive: true });
    writeFileSync(join(dir, file), contents);
  }
}

/** Writes a strategy file of the `thing` entity type. */
export function writeStrategy(dir: string, file: string, source: string): void {
  const path = join(dir, "entities/thing/strategies", file);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, source);
}

/** Writes one of the project's extractors. */
export function writeExtractor(dir: string, file: string, source: string): void {
  const path = join(dir, "extractors", file);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, source);
}

/** Whether `uv`, which runs Python strategies and extractors, is here. */
export const hasUv = Bun.which(process.env.SPOOL_UV ?? "uv") !== null;

export interface Server {
  url: string;
  /** A client with the server's token: an agent, not a person. */
  client: SpoolClient;
  /** The code of the link the server printed, which makes the first account. */
  setupCode: string;
  stop(): Promise<void>;
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

/**
 * Builds the project in `project` and serves it on a new store, on a port
 * the server picks. `seed` makes the IDs it mints reproducible.
 */
export async function serve(project: string, seed: number): Promise<Server> {
  spool("build", project);
  const dir = temporaryDirectory();
  const child = Bun.spawn(
    [
      binary(),
      "serve",
      "--manifest",
      join(project, ".spool/manifest.json"),
      "--project",
      project,
      "--tasks",
      join(dir, "tasks"),
      "--store",
      join(dir, "store.duckdb"),
      "--listen",
      "127.0.0.1:0",
      "--seed",
      String(seed),
    ],
    { env: { ...process.env, SPOOL_TOKEN: TOKEN }, stdout: "pipe", stderr: "inherit" },
  );

  // The server prints the address it listens on, and then, as nobody has
  // an account yet, the link that makes the first one.
  const reader = child.stdout.getReader();
  const decoder = new TextDecoder();
  let printed = "";
  let url: string | undefined;
  let setupCode: string | undefined;
  while (url === undefined || setupCode === undefined) {
    const { value, done } = await reader.read();
    if (done) throw new Error(`the server stopped before it listened:\n${printed}`);
    printed += decoder.decode(value, { stream: true });
    url = /listening on (http:\/\/\S+)/.exec(printed)?.[1];
    setupCode = /\/setup#(\w+)\n/.exec(printed)?.[1];
  }
  reader.releaseLock();

  return {
    url,
    client: new SpoolClient({ url, token: TOKEN }),
    setupCode,
    async stop() {
      child.kill("SIGTERM");
      await child.exited;
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

/** Copies a project's files, so that building it leaves the original alone. */
export function copyProject(from: string): string {
  const to = temporaryDirectory();
  for (const part of ["sources", "transforms", "entities", "extractors"]) {
    if (existsSync(join(from, part))) cpSync(join(from, part), join(to, part), { recursive: true });
  }
  return to;
}

/**
 * Reads a CSV file with a header row into one object per row. Every value is
 * text, and an empty field is left out, which is how the engine reads CSV.
 */
export function readCsv(path: string): Array<Record<string, string>> {
  const text = readFileSync(path, "utf8");
  const records: string[][] = [];
  let record: string[] = [];
  let field = "";
  let quoted = false;
  for (let index = 0; index < text.length; index++) {
    const char = text[index]!;
    if (quoted) {
      if (char === '"' && text[index + 1] === '"') {
        field += '"';
        index++;
      } else if (char === '"') {
        quoted = false;
      } else {
        field += char;
      }
    } else if (char === '"') {
      quoted = true;
    } else if (char === ",") {
      record.push(field);
      field = "";
    } else if (char === "\n" || char === "\r") {
      if (char === "\r" && text[index + 1] === "\n") index++;
      record.push(field);
      records.push(record);
      record = [];
      field = "";
    } else {
      field += char;
    }
  }
  if (field !== "" || record.length > 0) {
    record.push(field);
    records.push(record);
  }

  const [header, ...rows] = records;
  if (header === undefined) return [];
  return rows
    .filter((row) => row.some((value) => value !== ""))
    .map((row) => {
      const object: Record<string, string> = {};
      header.forEach((name, column) => {
        const value = row[column];
        if (value !== undefined && value !== "") object[name] = value;
      });
      return object;
    });
}
