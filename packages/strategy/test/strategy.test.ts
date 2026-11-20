// The helper against task directories written the way the engine writes them.

import { afterAll, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { parquetReadObjects } from "hyparquet";
import { parquetWriteBuffer } from "hyparquet-writer";

import { PROTOCOL, Task, strategy } from "../src/index.ts";
import type { Judgement } from "../src/index.ts";

const cleanup: string[] = [];
afterAll(() => {
  for (const dir of cleanup) rmSync(dir, { recursive: true, force: true });
});

/** A task directory with two records and the one pair of them. */
function taskDirectory(protocol = PROTOCOL): string {
  const dir = mkdtempSync(join(tmpdir(), "spool-strategy-"));
  cleanup.push(dir);
  writeFileSync(
    join(dir, "task.json"),
    JSON.stringify({
      protocol,
      run: 7,
      entity_type: "thing",
      declaration_version: 1,
      rematch: false,
      strategy: { name: "model", version: "abc" },
      features: [
        { name: "record_id", data_type: "UUID" },
        { name: "name", data_type: "VARCHAR" },
      ],
      inputs: { candidates: "candidates.parquet", features: "features.parquet" },
      output: "judgements.parquet",
    }),
  );
  const write = (file: string, columnData: Parameters<typeof parquetWriteBuffer>[0]["columnData"]) =>
    writeFileSync(join(dir, file), new Uint8Array(parquetWriteBuffer({ columnData })));
  write("candidates.parquet", [
    { name: "left_id", data: ["a"], type: "STRING" },
    { name: "right_id", data: ["b"], type: "STRING" },
    { name: "left_name", data: ["anvil"], type: "STRING" },
    { name: "right_name", data: ["Anvil"], type: "STRING" },
  ]);
  write("features.parquet", [
    { name: "record_id", data: ["a", "b"], type: "STRING" },
    { name: "is_new", data: [true, false], type: "BOOLEAN" },
    { name: "identity_id", data: [null, "i-1"], type: "STRING", nullable: true },
    { name: "name", data: ["anvil", "Anvil"], type: "STRING" },
  ]);
  return dir;
}

async function written(dir: string): Promise<Array<Record<string, unknown>>> {
  const bytes = readFileSync(join(dir, "judgements.parquet"));
  return parquetReadObjects({ file: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
}

/** Runs `strategy` as the engine starts it: with the task named in the environment. */
async function run(dir: string | undefined, judge: Parameters<typeof strategy>[0]): Promise<void> {
  const before = process.env.SPOOL_TASK_DIR;
  if (dir === undefined) delete process.env.SPOOL_TASK_DIR;
  else process.env.SPOOL_TASK_DIR = dir;
  try {
    await strategy(judge);
  } finally {
    if (before === undefined) delete process.env.SPOOL_TASK_DIR;
    else process.env.SPOOL_TASK_DIR = before;
  }
}

async function failure(call: Promise<unknown>): Promise<string> {
  try {
    await call;
  } catch (error) {
    return (error as Error).message;
  }
  throw new Error("the call succeeded");
}

describe("a task", () => {
  test("is read and its judgements are written where the engine reads them", async () => {
    const dir = taskDirectory();
    await run(dir, async (task) => {
      expect(task.run).toBe(7);
      expect(task.entityType).toBe("thing");
      expect(task.strategy).toBe("model");
      expect(task.rematch).toBe(false);
      expect(await task.features()).toEqual([
        { record_id: "a", is_new: true, identity_id: null, name: "anvil" },
        { record_id: "b", is_new: false, identity_id: "i-1", name: "Anvil" },
      ]);
      const [pair] = await task.candidates();
      expect(pair).toEqual({ left_id: "a", right_id: "b", left_name: "anvil", right_name: "Anvil" });
      return [
        {
          left_id: pair!.left_id,
          right_id: pair!.right_id,
          verdict: "match",
          score: 0.75,
          detail: { why: ["name"], n: 2n },
        },
        { left_id: "a", right_id: "c", verdict: "unsure" },
        { left_id: "a", right_id: "d", verdict: "no_match", score: null, detail: '{"given":"as text"}' },
      ];
    });
    expect(await written(dir)).toEqual([
      { left_id: "a", right_id: "b", verdict: "match", score: 0.75, detail: '{"why":["name"],"n":"2"}' },
      { left_id: "a", right_id: "c", verdict: "unsure", score: null, detail: null },
      { left_id: "a", right_id: "d", verdict: "no_match", score: null, detail: '{"given":"as text"}' },
    ]);
    expect(existsSync(join(dir, "judgements.parquet.part"))).toBe(false);
  });

  test("with no judgements still gets its file", async () => {
    const dir = taskDirectory();
    await run(dir, () => {});
    expect(await written(dir)).toEqual([]);
  });

  test("of another protocol is refused", async () => {
    const message = await failure(run(taskDirectory(PROTOCOL + 1), () => []));
    expect(message).toContain(`protocol ${PROTOCOL + 1}`);
  });
});

describe("judgements", () => {
  const refused = async (judgements: unknown): Promise<string> => {
    const dir = taskDirectory();
    const message = await failure(run(dir, () => judgements as Judgement[]));
    expect(existsSync(join(dir, "judgements.parquet"))).toBe(false);
    return message;
  };

  test("name what they lack and what they have too much of", async () => {
    const message = await refused([{ left_id: "a", verdict: "match", confidence: 1 }]);
    expect(message).toContain("row 0 has no `right_id` and also `confidence`");
  });

  test("have numbers for scores", async () => {
    expect(await refused([{ left_id: "a", right_id: "b", verdict: "match", score: "high" }])).toContain(
      "a score is a number, and row 0 has a string",
    );
  });

  test("are objects", async () => {
    expect(await refused([["a", "b", "match"]])).toContain("row 0 is an array");
  });
});

test("without a task the strategy does nothing, so its file can be imported", async () => {
  let called = false;
  await run(undefined, () => {
    called = true;
  });
  expect(called).toBe(false);
  const task = await Task.open(taskDirectory());
  expect(task.info.output).toBe("judgements.parquet");
});
