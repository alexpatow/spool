// What a Spool strategy written in TypeScript imports.
//
// A strategy is a file under a project's `strategies/` directory. Spool runs
// it as a process of its own, hands it a directory of Parquet files, and
// reads the judgements it leaves there. This module hides the files:
//
//   import { strategy } from "spool-strategy";
//
//   await strategy(async (task) => {
//     const pairs = await task.candidates();
//     return pairs
//       .filter((pair) => pair.left_name === pair.right_name)
//       .map((pair) => ({ left_id: pair.left_id, right_id: pair.right_id, verdict: "match" }));
//   });
//
// A strategy says whether pairs are the same thing. It cannot change an
// identity: Spool reconciles the judgements and commits the result itself.
//
// Without `SPOOL_TASK_DIR` in the environment `strategy` does nothing, so the
// file can be imported, and its functions called, in a test.
//
// Spool ships this module as one file with the Parquet reader and writer in
// it: `bun run build` writes that file, and the engine puts it where `bun`
// finds it. A strategy needs nothing installed to import it.

import { readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { parquetReadObjects } from "hyparquet";
import { parquetWriteBuffer } from "hyparquet-writer";

/** The version of the task directory's layout this module reads and writes. */
export const PROTOCOL = 1;

export type Verdict = "match" | "no_match" | "unsure";

/** What a strategy says about two observations. */
export interface Judgement {
  left_id: string;
  right_id: string;
  verdict: Verdict;
  /** A number on the strategy's own scale. */
  score?: number | null;
  /** Anything JSON can hold. It is kept with the judgement. */
  detail?: unknown;
}

/**
 * A row of a Parquet file. A date or a timestamp is a `Date`, and a 64-bit
 * integer is a `bigint`.
 */
export type Row = Record<string, unknown>;

/**
 * A pair the project's block rules found: both IDs, and every feature column
 * of both sides as `left_<name>` and `right_<name>`.
 */
export interface CandidatePair extends Row {
  left_id: string;
  right_id: string;
}

/**
 * One observation of the entity type with its feature columns. A strategy
 * that finds its own pairs starts here.
 */
export interface FeatureRow extends Row {
  observation_id: string;
  /** Whether the run is to place the observation. */
  is_new: boolean;
  /** `null` for an observation that has no identity. */
  identity_id: string | null;
}

/** `task.json`, as Spool wrote it. */
export interface TaskInfo {
  protocol: number;
  run: number;
  entity_type: string;
  declaration_version: number;
  rematch: boolean;
  strategy: { name: string; version: string };
  features: Array<{ name: string; data_type: string }>;
  inputs: { candidates: string; features: string };
  output: string;
}

const COLUMNS = ["left_id", "right_id", "verdict", "score", "detail"];
const REQUIRED = ["left_id", "right_id", "verdict"] as const;

/** One run's task: what Spool wrote, and where the judgements go. */
export class Task {
  /** The match run the judgements belong to. */
  readonly run: number;
  readonly entityType: string;
  /** The strategy's name: the file's name without `.ts`. */
  readonly strategy: string;
  /**
   * True when the run examines every observation again, not only those that
   * are new.
   */
  readonly rematch: boolean;
  private readonly tables = new Map<string, Promise<Row[]>>();

  private constructor(
    readonly dir: string,
    readonly info: TaskInfo,
  ) {
    this.run = info.run;
    this.entityType = info.entity_type;
    this.strategy = info.strategy.name;
    this.rematch = info.rematch === true;
  }

  static async open(dir: string): Promise<Task> {
    const info = JSON.parse(await readFile(join(dir, "task.json"), "utf8")) as TaskInfo;
    if (info.protocol !== PROTOCOL) {
      throw new Error(
        `this task was written with protocol ${info.protocol}, and this helper speaks protocol ${PROTOCOL}`,
      );
    }
    return new Task(dir, info);
  }

  private read(name: keyof TaskInfo["inputs"]): Promise<Row[]> {
    let table = this.tables.get(name);
    if (table === undefined) {
      table = readFile(join(this.dir, this.info.inputs[name])).then((bytes) => {
        const file = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
        return parquetReadObjects({ file });
      });
      this.tables.set(name, table);
    }
    return table;
  }

  /** The candidate pairs with both sides' features. */
  candidates(): Promise<CandidatePair[]> {
    return this.read("candidates") as Promise<CandidatePair[]>;
  }

  /** Every observation of the entity type with its features. */
  features(): Promise<FeatureRow[]> {
    return this.read("features") as Promise<FeatureRow[]>;
  }

  /** Writes the run's judgements. Whatever was written before is replaced. */
  async write(judgements: Iterable<Judgement>): Promise<void> {
    const rows = [...judgements];
    rows.forEach(check);
    const text = (name: (typeof REQUIRED)[number]) => ({
      name,
      data: rows.map((row) => String(row[name])),
      type: "STRING" as const,
    });
    const buffer = parquetWriteBuffer({
      columnData: [
        text("left_id"),
        text("right_id"),
        text("verdict"),
        { name: "score", data: rows.map((row) => row.score ?? null), type: "DOUBLE", nullable: true },
        { name: "detail", data: rows.map((row) => detail(row.detail)), type: "STRING", nullable: true },
      ],
    });
    // Written under another name and then renamed, so that Spool never reads
    // half a file.
    const path = join(this.dir, this.info.output);
    await writeFile(`${path}.part`, new Uint8Array(buffer));
    await rename(`${path}.part`, path);
  }
}

function check(row: Judgement, index: number): void {
  if (row === null || typeof row !== "object" || Array.isArray(row)) {
    throw new TypeError(`a strategy returns an array of objects, and row ${index} is ${describe(row)}`);
  }
  const names = Object.keys(row);
  const missing = REQUIRED.filter((name) => row[name] === undefined || row[name] === null);
  const unknown = names.filter((name) => !COLUMNS.includes(name));
  if (missing.length > 0 || unknown.length > 0) {
    const problems: string[] = [];
    if (missing.length > 0) problems.push(`no ${missing.map((name) => `\`${name}\``).join(", ")}`);
    if (unknown.length > 0) problems.push(`also ${unknown.map((name) => `\`${name}\``).join(", ")}`);
    throw new TypeError(
      "judgements have `left_id`, `right_id` and `verdict`, and optionally `score` and `detail`; " +
        `row ${index} has ${problems.join(" and ")}`,
    );
  }
  if (row.score !== undefined && row.score !== null && typeof row.score !== "number") {
    throw new TypeError(`a score is a number, and row ${index} has ${describe(row.score)}`);
  }
}

function describe(value: unknown): string {
  if (value === null) return "null";
  if (Array.isArray(value)) return "an array";
  return `a ${typeof value}`;
}

/** The detail as JSON text. A string is taken to be JSON already. */
function detail(value: unknown): string | null {
  if (value === undefined || value === null) return null;
  if (typeof value === "string") return value;
  return JSON.stringify(value, (_, inner) => (typeof inner === "bigint" ? inner.toString() : inner));
}

/**
 * Runs `judge` as this file's strategy, and writes the judgements it
 * returns. Returning nothing means no judgements. At least one side of every
 * pair must be new to the run.
 *
 * Await it at the top of the file. If `judge` throws, the process ends with
 * the error, and the run fails with it.
 */
export async function strategy(
  judge: (task: Task) => Iterable<Judgement> | void | Promise<Iterable<Judgement> | void>,
): Promise<void> {
  const dir = process.env.SPOOL_TASK_DIR;
  if (!dir) return;
  const task = await Task.open(dir);
  await task.write((await judge(task)) ?? []);
}
