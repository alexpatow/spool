// What a Spool strategy or extractor written in TypeScript imports.
//
// A strategy is a file under an entity type's `strategies/` directory, and an
// extractor is a file under the project's `extractors/` directory. Spool runs
// each as a process of its own, hands it a directory of Parquet files, and
// reads what it leaves there. This module hides the files.
//
// A strategy says which pairs of observations are the same thing:
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
// An extractor finds the things documents mention:
//
//   import { extractor } from "spool-strategy";
//
//   await extractor(async (task) => {
//     const documents = await task.documents();
//     return documents.flatMap((document) =>
//       [...document.text.matchAll(/[A-Z][a-z]+ (Inc|Ltd)/g)].map((found) =>
//         document.mention("company", found.index, found.index + found[0].length, { name: found[0] }),
//       ),
//     );
//   });
//
// Each mention becomes an observation of its entity type, whose source is the
// document's. An extractor says what a document mentions. It cannot say that
// two mentions are the same thing: the type's rules and strategies decide
// that.
//
// Without `SPOOL_TASK_DIR` in the environment `strategy` and `extractor` do
// nothing, so the file can be imported, and its functions called, in a test.
//
// Spool ships this module as one file with the Parquet reader and writer in
// it: `bun run build` writes that file, and the engine puts it where `bun`
// finds it. A strategy or an extractor needs nothing installed to import it.

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

/** `task.json` of a match run, as Spool wrote it. */
export interface TaskInfo {
  protocol: number;
  kind?: "match";
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
    return new Task(dir, (await taskInfo(dir, "match", "strategy")) as TaskInfo);
  }

  private read(name: keyof TaskInfo["inputs"]): Promise<Row[]> {
    let table = this.tables.get(name);
    if (table === undefined) {
      table = readRows(join(this.dir, this.info.inputs[name]));
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
    await writeWhole(join(this.dir, this.info.output), buffer);
  }
}

/** Reads `task.json` and checks that the task is one for `caller`. */
async function taskInfo(dir: string, kind: "match" | "extract", caller: string): Promise<unknown> {
  const info = JSON.parse(await readFile(join(dir, "task.json"), "utf8")) as { protocol: number; kind?: string };
  if (info.protocol !== PROTOCOL) {
    throw new Error(
      `this task was written with protocol ${info.protocol}, and this helper speaks protocol ${PROTOCOL}`,
    );
  }
  const found = info.kind ?? "match";
  if (found !== kind) {
    throw new Error(
      `\`${caller}\` was given a task of kind \`${found}\`: a strategy belongs under an entity type's ` +
        "`strategies/` directory and an extractor under the project's `extractors/` directory",
    );
  }
  return info;
}

async function readRows(path: string): Promise<Row[]> {
  const bytes = await readFile(path);
  const file = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  return parquetReadObjects({ file });
}

/**
 * Writes a file under another name and then renames it, so that Spool never
 * reads half a file.
 */
async function writeWhole(path: string, buffer: ArrayBuffer): Promise<void> {
  await writeFile(`${path}.part`, new Uint8Array(buffer));
  await rename(`${path}.part`, path);
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

/** What an attribute of a mention may be. One without a value is left out. */
export type Attribute = string | number | boolean | null | undefined;

/**
 * Something a document mentions. `document.mention(...)` makes one from
 * positions in the document's text as JavaScript counts them.
 */
export interface Mention {
  document_id: string;
  entity_type: string;
  /**
   * The position of the mention's first character, counted in Unicode code
   * points from the start of the text. This is not an index into a
   * JavaScript string wherever the text has a character outside the Basic
   * Multilingual Plane, which a string holds as two units.
   */
  start: number;
  /** The position after the mention's last character, counted the same way. */
  end: number;
  /** What is known about the thing at this place. */
  attributes?: Record<string, Attribute> | null;
}

/** `task.json` of an extract run, as Spool wrote it. */
export interface ExtractTaskInfo {
  protocol: number;
  kind: "extract";
  run: number;
  extractor: { name: string; version: string };
  entity_types: Array<{
    name: string;
    instructions: string | null;
    features: Array<{ name: string; data_type: string }>;
  }>;
  inputs: { documents: string };
  output: string;
}

/** Marks an index into a string that is the second half of a character. */
const INSIDE = 0xffffffff;

/** A document an extractor reads. */
export class Document {
  private points: Uint32Array | undefined;

  constructor(
    /** The ID of this version of the document. */
    readonly id: string,
    /** The source the document came from. */
    readonly source: string,
    /** The document's key within its source. */
    readonly key: string,
    /** Where the source keeps the document, if it said. */
    readonly uri: string | null,
    readonly text: string,
  ) {}

  /**
   * A mention of an `entityType` at `text.slice(start, end)`. `start` and
   * `end` are indices into `text`, as a match of a regular expression or
   * `indexOf` gives them.
   */
  mention(entityType: string, start: number, end: number, attributes: Record<string, Attribute> = {}): Mention {
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || start >= end || end > this.text.length) {
      throw new RangeError(
        `${start}-${end} is not a part of the text of document \`${this.key}\`, which is ${this.text.length} long`,
      );
    }
    return {
      document_id: this.id,
      entity_type: entityType,
      start: this.point(start),
      end: this.point(end),
      attributes,
    };
  }

  /**
   * Where `quote` is in the text: a `[start, end]` for each place, in order,
   * to hand to `mention`. For an extractor that is told what a document
   * mentions and not where, as a language model tells it. A quote that is
   * not in the text, letter for letter, is found nowhere.
   */
  find(quote: string): Array<[number, number]> {
    const places: Array<[number, number]> = [];
    if (quote.length === 0) return places;
    let at = this.text.indexOf(quote);
    while (at !== -1) {
      places.push([at, at + quote.length]);
      at = this.text.indexOf(quote, at + quote.length);
    }
    return places;
  }

  /** How many code points come before an index into the text. */
  private point(index: number): number {
    if (this.points === undefined) {
      const text = this.text;
      const points = new Uint32Array(text.length + 1);
      let count = 0;
      for (let at = 0; at < text.length; at++) {
        points[at] = count++;
        const unit = text.charCodeAt(at);
        const next = text.charCodeAt(at + 1);
        if (unit >= 0xd800 && unit <= 0xdbff && next >= 0xdc00 && next <= 0xdfff) {
          points[++at] = INSIDE;
        }
      }
      points[text.length] = count;
      this.points = points;
    }
    const point = this.points[index]!;
    if (point === INSIDE) {
      throw new RangeError(`position ${index} of document \`${this.key}\` is in the middle of a character`);
    }
    return point;
  }
}

/** One extract run's task: the documents, and where the mentions go. */
export class ExtractTask {
  /** The extract run the mentions belong to. */
  readonly run: number;
  /** The extractor's name: the file's name without `.ts`. */
  readonly extractor: string;
  /** The names of the entity types the project declares. A mention names one of them. */
  readonly entityTypes: string[];
  private read: Promise<Document[]> | undefined;

  private constructor(
    readonly dir: string,
    readonly info: ExtractTaskInfo,
  ) {
    this.run = info.run;
    this.extractor = info.extractor.name;
    this.entityTypes = info.entity_types.map((entry) => entry.name);
  }

  static async open(dir: string): Promise<ExtractTask> {
    return new ExtractTask(dir, (await taskInfo(dir, "extract", "extractor")) as ExtractTaskInfo);
  }

  private entityType(name: string): ExtractTaskInfo["entity_types"][number] {
    const entry = this.info.entity_types.find((entry) => entry.name === name);
    if (entry === undefined) throw new Error(`the project declares no entity type \`${name}\``);
    return entry;
  }

  /** What the type's `extract.md` says to look for, or `null`. */
  instructions(entityType: string): string | null {
    return this.entityType(entityType).instructions ?? null;
  }

  /** The type's feature columns. */
  features(entityType: string): Array<{ name: string; data_type: string }> {
    return this.entityType(entityType).features;
  }

  /** The documents to read. */
  documents(): Promise<Document[]> {
    this.read ??= readRows(join(this.dir, this.info.inputs.documents)).then((rows) =>
      rows.map(
        (row) =>
          new Document(
            row.document_id as string,
            row.source as string,
            row.key as string,
            (row.uri as string | null | undefined) ?? null,
            row.text as string,
          ),
      ),
    );
    return this.read;
  }

  /** Writes the run's mentions. Whatever was written before is replaced. */
  async write(mentions: Iterable<Mention>): Promise<void> {
    const rows = [...mentions];
    rows.forEach(checkMention);
    const buffer = parquetWriteBuffer({
      columnData: [
        { name: "document_id", data: rows.map((row) => String(row.document_id)), type: "STRING" },
        { name: "entity_type", data: rows.map((row) => String(row.entity_type)), type: "STRING" },
        { name: "start", data: rows.map((row) => row.start), type: "INT32" },
        { name: "end", data: rows.map((row) => row.end), type: "INT32" },
        { name: "attributes", data: rows.map((row) => detail(row.attributes)), type: "STRING", nullable: true },
      ],
    });
    await writeWhole(join(this.dir, this.info.output), buffer);
  }
}

const MENTION = ["document_id", "entity_type", "start", "end", "attributes"];

function checkMention(row: Mention, index: number): void {
  if (row === null || typeof row !== "object" || Array.isArray(row)) {
    throw new TypeError(
      `an extractor returns mentions, as \`document.mention(...)\` makes them, and mention ${index} is ${describe(row)}`,
    );
  }
  const given = row as unknown as Record<string, unknown>;
  const missing = MENTION.slice(0, 4).filter((name) => given[name] === undefined || given[name] === null);
  const unknown = Object.keys(given).filter((name) => !MENTION.includes(name));
  if (missing.length > 0 || unknown.length > 0) {
    const problems: string[] = [];
    if (missing.length > 0) problems.push(`no ${missing.map((name) => `\`${name}\``).join(", ")}`);
    if (unknown.length > 0) problems.push(`also ${unknown.map((name) => `\`${name}\``).join(", ")}`);
    throw new TypeError(
      "a mention has `document_id`, `entity_type`, `start` and `end`, and optionally `attributes`; " +
        `mention ${index} has ${problems.join(" and ")}`,
    );
  }
  for (const name of ["start", "end"] as const) {
    if (!Number.isInteger(row[name])) {
      throw new TypeError(`\`${name}\` is a position in the document's text, and mention ${index} has ${describe(row[name])}`);
    }
  }
  const attributes = row.attributes;
  if (attributes !== undefined && attributes !== null && (typeof attributes !== "object" || Array.isArray(attributes))) {
    throw new TypeError(`\`attributes\` is an object, and mention ${index} has ${describe(attributes)}`);
  }
}

/**
 * Runs `find` as this file's extractor, and writes the mentions it returns.
 * Returning nothing means the documents mention nothing.
 *
 * Await it at the top of the file. If `find` throws, the process ends with
 * the error, the run fails with it, and nothing it found is stored.
 */
export async function extractor(
  find: (task: ExtractTask) => Iterable<Mention> | void | Promise<Iterable<Mention> | void>,
): Promise<void> {
  const dir = process.env.SPOOL_TASK_DIR;
  if (!dir) return;
  const task = await ExtractTask.open(dir);
  await task.write((await find(task)) ?? []);
}
