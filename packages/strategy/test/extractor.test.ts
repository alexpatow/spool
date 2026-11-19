// The helper against extraction tasks written the way the engine writes them.

import { afterAll, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { parquetReadObjects } from "hyparquet";
import { parquetWriteBuffer } from "hyparquet-writer";

import { Document, PROTOCOL, extractor, strategy } from "../src/index.ts";
import type { Mention } from "../src/index.ts";

const cleanup: string[] = [];
afterAll(() => {
  for (const dir of cleanup) rmSync(dir, { recursive: true, force: true });
});

const FIRST = "🚀 Acme Inc opened an office. Globex Ltd did not.";
const SECOND = "Société Ltd bought Acme Inc.";

/** A task directory with two documents. */
function taskDirectory(kind = "extract"): string {
  const dir = mkdtempSync(join(tmpdir(), "spool-extractor-"));
  cleanup.push(dir);
  writeFileSync(
    join(dir, "task.json"),
    JSON.stringify({
      protocol: PROTOCOL,
      kind,
      run: 9,
      extractor: { name: "names", version: "abc" },
      entity_types: [
        {
          name: "company",
          instructions: "A company is named with its legal form.",
          features: [{ name: "name", data_type: "VARCHAR" }],
        },
        { name: "person", instructions: null, features: [] },
      ],
      inputs: { documents: "documents.parquet" },
      output: "mentions.parquet",
      failed: "failed.parquet",
    }),
  );
  const buffer = parquetWriteBuffer({
    columnData: [
      { name: "document_id", data: ["d-1", "d-2"], type: "STRING" },
      { name: "source", data: ["news", "news"], type: "STRING" },
      { name: "key", data: ["a", "b"], type: "STRING" },
      { name: "uri", data: ["https://news.test/a", null], type: "STRING", nullable: true },
      { name: "text", data: [FIRST, SECOND], type: "STRING" },
    ],
  });
  writeFileSync(join(dir, "documents.parquet"), new Uint8Array(buffer));
  return dir;
}

async function written(dir: string): Promise<Array<Record<string, unknown>>> {
  const bytes = readFileSync(join(dir, "mentions.parquet"));
  return parquetReadObjects({ file: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
}

/** Runs `call` as the engine starts an extractor: with the task named in the environment. */
async function within<T>(dir: string | undefined, call: () => Promise<T>): Promise<T> {
  const before = process.env.SPOOL_TASK_DIR;
  if (dir === undefined) delete process.env.SPOOL_TASK_DIR;
  else process.env.SPOOL_TASK_DIR = dir;
  try {
    return await call();
  } finally {
    if (before === undefined) delete process.env.SPOOL_TASK_DIR;
    else process.env.SPOOL_TASK_DIR = before;
  }
}

const run = (dir: string | undefined, find: Parameters<typeof extractor>[0]) => within(dir, () => extractor(find));

async function failure(call: Promise<unknown>): Promise<string> {
  try {
    await call;
  } catch (error) {
    return (error as Error).message;
  }
  throw new Error("the call succeeded");
}

describe("an extraction task", () => {
  test("is read and its mentions are written where the engine reads them", async () => {
    const dir = taskDirectory();
    await run(dir, async (task) => {
      expect(task.run).toBe(9);
      expect(task.extractor).toBe("names");
      expect(task.entityTypes).toEqual(["company", "person"]);
      expect(task.instructions("company")).toBe("A company is named with its legal form.");
      expect(task.instructions("person")).toBeNull();
      expect(task.features("company")).toEqual([{ name: "name", data_type: "VARCHAR" }]);
      expect(() => task.instructions("place")).toThrow("no entity type `place`");
      const documents = await task.documents();
      expect(documents.map((document) => [document.id, document.source, document.key, document.uri])).toEqual([
        ["d-1", "news", "a", "https://news.test/a"],
        ["d-2", "news", "b", null],
      ]);
      return documents.flatMap((document) =>
        [...document.text.matchAll(/[A-Z]\p{L}+ (?:Inc|Ltd)/gu)].map((found) =>
          document.mention("company", found.index, found.index + found[0].length, {
            name: found[0],
            listed: undefined,
          }),
        ),
      );
    });
    // Positions count code points: the rocket is two units of a JavaScript
    // string and one character of the document.
    expect(await written(dir)).toEqual([
      { document_id: "d-1", entity_type: "company", start: 2, end: 10, attributes: '{"name":"Acme Inc"}' },
      { document_id: "d-1", entity_type: "company", start: 29, end: 39, attributes: '{"name":"Globex Ltd"}' },
      { document_id: "d-2", entity_type: "company", start: 0, end: 11, attributes: '{"name":"Société Ltd"}' },
      { document_id: "d-2", entity_type: "company", start: 19, end: 27, attributes: '{"name":"Acme Inc"}' },
    ]);
    expect(existsSync(join(dir, "mentions.parquet.part"))).toBe(false);
  });

  test("takes its mentions from a generator, which gives them only once", async () => {
    const dir = taskDirectory();
    await run(dir, async (task) => {
      const [first] = await task.documents();
      return (function* () {
        yield first!.mention("company", 3, 11, { name: "Acme Inc" });
        yield first!.mention("company", 30, 40, { name: "Globex Ltd" });
      })();
    });
    expect((await written(dir)).map((row) => row.start)).toEqual([2, 29]);
  });

  test("writes the documents it could not read, and a write replaces them", async () => {
    const dir = taskDirectory();
    await run(dir, async (task) => {
      const [first, second] = await task.documents();
      return [first!.mention("company", 3, 11, { name: "Acme Inc" }), second!.failed("the model answered 500")];
    });
    expect(await written(dir)).toHaveLength(1);
    const bytes = readFileSync(join(dir, "failed.parquet"));
    expect(
      await parquetReadObjects({ file: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) }),
    ).toEqual([{ document_id: "d-2", reason: "the model answered 500" }]);

    // Written again with the document read, it is not failed any more.
    await run(dir, async (task) => {
      const [, second] = await task.documents();
      return [second!.mention("company", 0, 11, { name: "Société Ltd" })];
    });
    expect(existsSync(join(dir, "failed.parquet"))).toBe(false);
    expect((await written(dir)).map((row) => row.document_id)).toEqual(["d-2"]);
  });

  test("with no mentions still gets its file", async () => {
    const dir = taskDirectory();
    await run(dir, () => {});
    expect(await written(dir)).toEqual([]);
  });

  test("is not a strategy's, and a strategy's is not an extractor's", async () => {
    expect(await failure(within(taskDirectory(), () => strategy(() => [])))).toContain(
      "`strategy` was given a task of kind `extract`",
    );
    expect(await failure(run(taskDirectory("match"), () => []))).toContain(
      "`extractor` was given a task of kind `match`",
    );
  });
});

describe("a document", () => {
  const document = new Document("d-1", "news", "a", null, FIRST);

  test("finds every place a quote is", () => {
    const twice = new Document("d-3", "news", "c", null, "Acme and Acme again 🚀 Acme");
    expect(twice.find("Acme")).toEqual([
      [0, 4],
      [9, 13],
      [23, 27],
    ]);
    expect(twice.find("acme")).toEqual([]);
    expect(twice.find("")).toEqual([]);
    const [, , last] = twice.find("Acme");
    expect(twice.mention("company", last![0], last![1])).toEqual({
      document_id: "d-3",
      entity_type: "company",
      start: 22,
      end: 26,
      attributes: {},
    });
  });

  test("refuses a place that is not in its text", () => {
    for (const [start, end] of [
      [-1, 3],
      [3, 3],
      [5, 3],
      [0, FIRST.length + 1],
      [0.5, 3],
    ]) {
      expect(() => document.mention("company", start!, end!)).toThrow("is not a part of the text");
    }
    // The rocket is the units 0 and 1: a mention cannot begin or end between them.
    expect(() => document.mention("company", 1, 5)).toThrow("in the middle of a character");
    expect(document.mention("company", 0, 2).end).toBe(1);
  });
});

describe("mentions", () => {
  const refused = async (mentions: unknown): Promise<string> => {
    const dir = taskDirectory();
    const message = await failure(run(dir, () => mentions as Mention[]));
    expect(existsSync(join(dir, "mentions.parquet"))).toBe(false);
    return message;
  };

  test("name what they lack and what they have too much of", async () => {
    const message = await refused([{ document_id: "d-1", entity_type: "company", start: 0, where: 4 }]);
    expect(message).toContain("mention 0 has no `end` and also `where`");
  });

  test("have whole numbers for positions and an object for attributes", async () => {
    expect(await refused([{ document_id: "d-1", entity_type: "company", start: "0", end: 4 }])).toContain(
      "`start` is a position in the document's text, and mention 0 has a string",
    );
    expect(
      await refused([{ document_id: "d-1", entity_type: "company", start: 0, end: 4, attributes: ["Acme"] }]),
    ).toContain("`attributes` is an object, and mention 0 has an array");
  });

  test("are objects", async () => {
    expect(await refused(["Acme Inc"])).toContain("mention 0 is a string");
  });
});

test("without a task the extractor does nothing, so its file can be imported", async () => {
  let called = false;
  await run(undefined, () => {
    called = true;
  });
  expect(called).toBe(false);
});
