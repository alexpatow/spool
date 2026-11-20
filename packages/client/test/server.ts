// What the client's tests give the real `spool` they call: the debug build
// of this checkout, and projects written for them. `packages/testing` finds
// the binary and starts the server.

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import { copyProject as copyProjectTo, makeHarness, makeScratchDirectory, repository } from "spool-testing";

export { PASSWORD, cookies, firstPerson, readCsv, repository } from "spool-testing";
export type { Browser, Server } from "spool-testing";

// These tests call the server and do not time it, so the build that is
// quickest to make will do: the debug build, unless `SPOOL_BIN` names a binary.
export const { binary, spool, serve } = makeHarness("debug");

export const companies = join(repository, "benchmarks/identity/companies-small");

export function temporaryDirectory(): string {
  return makeScratchDirectory("spool-client");
}

/** Copies a project's files, so that building it leaves the original alone. */
export function copyProject(from: string): string {
  return copyProjectTo(from, "spool-client");
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
