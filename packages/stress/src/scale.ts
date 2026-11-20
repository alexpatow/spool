// How a server does as the store grows: generated company records are
// loaded and matched over HTTP, in batches, and each step is timed.
//
//   bun src/scale.ts 10000 100000        the numbers of true companies to try
//   bun src/scale.ts --out results.json 50000
//
// A set has about 2.2 records for each company. After the batches, every
// record is matched again, the calls a reader makes are timed, the server
// is stopped, and the store is verified.

import { rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import type { RunEntry, SpoolClient } from "spool-client";

import { generate, records } from "./generate.ts";
import type { Row } from "./generate.ts";
import { scratch, serve, storeSize, timed, verify } from "./server.ts";
import { writeProject } from "./write.ts";

/** The most rows one call of `POST /v1/load` carries. */
const CHUNK = 100_000;

export interface Step {
  step: string;
  seconds: number;
  /** Records the store holds after the step. */
  records: number;
  identities: number;
  unassigned: number;
  review: number;
  /** The log position after the step. */
  head: number;
  memory_mb: number;
  store_mb: number;
  detail?: string;
}

export interface Reads {
  [call: string]: { calls: number; median_ms: number; worst_ms: number };
}

export interface ScaleResult {
  companies: number;
  records: number;
  steps: Step[];
  reads: Reads;
  verified: string;
  peak_memory_mb: number;
}

export async function finished(client: SpoolClient, run: RunEntry | null): Promise<RunEntry | null> {
  if (run === null || run.status !== "running") return run;
  return await client.waitForRun(run.run);
}

/**
 * Matches the companies and waits for the run. Null when the server had
 * nothing to match and started no run: nothing is new since the last run,
 * and nothing waits for review.
 */
export async function match(client: SpoolClient, rematch = false): Promise<RunEntry | null> {
  const run = await finished(client, await client.match({ entity_type: "company", rematch }));
  if (run !== null && run.status !== "succeeded") throw new Error(`the match run ${run.status}: ${run.error ?? ""}`);
  return run;
}

/** A row as the engine reads it from a CSV file: an empty field is left out. */
export function filled(row: Row): Row {
  return Object.fromEntries(Object.entries(row).filter(([, value]) => value !== ""));
}

/** Sends rows in calls of at most `CHUNK`, waits for each, and says what became of them. */
export async function load(client: SpoolClient, source: string, rows: Row[]): Promise<string> {
  let inserted = 0;
  let duplicates = 0;
  let superseded = 0;
  for (let from = 0; from < rows.length; from += CHUNK) {
    const run = await finished(
      client,
      await client.load({ source, entity_type: "company", rows: rows.slice(from, from + CHUNK).map(filled) }),
    );
    if (run?.status !== "succeeded") throw new Error(`load ${run?.status}: ${run?.error ?? ""}`);
    inserted += run.load?.inserted ?? 0;
    duplicates += run.load?.duplicates ?? 0;
    superseded += run.load?.superseded ?? 0;
  }
  return `${inserted} new, ${duplicates} known, ${superseded} superseded`;
}

function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)] ?? 0;
}

export async function scale(
  companies: number,
  seed = 1,
  batches = 3,
  log: (line: string) => void = console.log,
): Promise<ScaleResult> {
  const dir = scratch(`scale-${companies}`);
  const generated = generate({ companies, seed, batches });
  const total = writeProject(dir, generated);
  const server = await serve({ project: dir, store: join(dir, "store.duckdb"), seed });
  const { client } = server;
  const steps: Step[] = [];
  let peak = 0;
  const watch = setInterval(() => (peak = Math.max(peak, server.memory())), 250);

  const record = async (step: string, seconds: number, detail?: string) => {
    const info = await client.info();
    const type = info.store.entity_types.find((type) => type.name === "company")!;
    peak = Math.max(peak, server.memory());
    const line: Step = {
      step,
      seconds: Math.round(seconds * 100) / 100,
      records: type.records,
      identities: type.identities,
      unassigned: type.unassigned,
      review: type.open_review_items,
      head: info.store.head,
      memory_mb: server.memory(),
      store_mb: storeSize(server.store),
      ...(detail === undefined ? {} : { detail }),
    };
    steps.push(line);
    log(
      `  ${step.padEnd(22)} ${line.seconds.toFixed(2).padStart(9)} s  ${String(line.records).padStart(9)} obs  ${String(
        line.identities,
      ).padStart(
        9,
      )} ids  ${String(line.memory_mb).padStart(6)} MB  ${String(line.store_mb).padStart(6)} MB  ${detail ?? ""}`,
    );
  };

  try {
    log(`${companies} companies, ${total} records, in ${dir}`);
    for (const [index, batch] of generated.batches.entries()) {
      const name = `batch ${index + 1}`;
      const [crm, crmSeconds] = await timed(() => load(client, "crm", batch.crm));
      await record(`${name}: load crm`, crmSeconds, `${batch.crm.length} rows: ${crm}`);
      const [registry, registrySeconds] = await timed(() => load(client, "registry", batch.registry));
      await record(`${name}: load registry`, registrySeconds, `${batch.registry.length} rows: ${registry}`);
      const [run, seconds] = await timed(() => match(client));
      const resolution = run?.resolution;
      await record(
        `${name}: match`,
        seconds,
        resolution == null
          ? "nothing to match"
          : `${resolution.candidates} candidates, ${resolution.identities_created} identities created, ${resolution.records_assigned} assigned`,
      );
    }

    const [rematch, rematchSeconds] = await timed(() => match(client, true));
    const suggestions = rematch?.resolution?.suggestions;
    await record(
      "rematch",
      rematchSeconds,
      `${rematch?.resolution?.candidates ?? 0} candidates, ${suggestions?.merges.length ?? 0} merges and ${suggestions?.splits.length ?? 0} splits suggested`,
    );

    const reads = await readCalls(client, generated.batches[0]!.crm);
    for (const [call, { calls, median_ms, worst_ms }] of Object.entries(reads)) {
      log(`  read ${call.padEnd(28)} ${calls} calls, median ${median_ms} ms, worst ${worst_ms} ms`);
    }

    clearInterval(watch);
    const [code, stopSeconds] = await timed(() => server.stop());
    if (code !== 0) throw new Error(`the server stopped with code ${code}:\n${server.complaints()}`);
    log(`  stopped in ${stopSeconds.toFixed(2)} s`);
    const verified = verify(server.store);
    log(`  ${verified}`);
    return { companies, records: records(generated), steps, reads, verified, peak_memory_mb: peak };
  } finally {
    clearInterval(watch);
    if (server.alive()) await server.kill();
    if (process.env.SPOOL_STRESS_KEEP === undefined) rmSync(dir, { recursive: true, force: true });
  }
}

/** Times the calls a reader makes, each a number of times. */
async function readCalls(client: SpoolClient, rows: Row[]): Promise<Reads> {
  const reads: Reads = {};
  const time = async (call: string, times: number, work: (index: number) => Promise<unknown>) => {
    const taken: number[] = [];
    for (let index = 0; index < times; index++) {
      const [, seconds] = await timed(() => work(index));
      taken.push(seconds * 1000);
    }
    reads[call] = {
      calls: times,
      median_ms: Math.round(median(taken) * 10) / 10,
      worst_ms: Math.round(Math.max(...taken) * 10) / 10,
    };
  };
  // Rows spread over the batch, so that they are not all neighbours.
  const sample = [...new Set(Array.from({ length: 20 }, (_, index) => rows[Math.floor((index * rows.length) / 20)]!))];
  const found: Array<{ id: string; identity: string | null }> = [];
  await time("info", 5, () => client.info());
  await time("record by source and key", sample.length, async (index) => {
    const { records } = await client.records({ source: "crm", key: sample[index]!.id });
    const record = records[0];
    if (record === undefined) throw new Error(`crm:${sample[index]!.id} is not in the store`);
    found.push({ id: record.id, identity: record.identity ?? null });
  });
  const placed = found.filter((record) => record.identity !== null);
  await time("record by ID", found.length, (index) => client.record(found[index]!.id));
  await time("identity", placed.length, (index) => client.identity(placed[index]!.identity!));
  await time("identity history", placed.length, (index) => client.identityHistory(placed[index]!.identity!));
  await time("identities, first page", 5, () => client.identities());
  await time("records, first page", 5, () => client.records());
  await time("explain a pair", Math.max(0, found.length - 1), (index) =>
    client.explain(found[index]!.id, found[index + 1]!.id),
  );
  await time("review items", 5, () => client.reviewItems());
  await time("runs", 5, () => client.runs());
  await time("changesets, first page", 3, () => client.changesets({ limit: 1 }));
  await time("constraints", 5, () => client.constraints());
  return reads;
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const out = args.includes("--out") ? args[args.indexOf("--out") + 1] : undefined;
  const sizes = args.filter((arg, index) => /^\d+$/.test(arg) && args[index - 1] !== "--out").map(Number);
  const results: ScaleResult[] = [];
  for (const companies of sizes.length > 0 ? sizes : [10_000]) {
    results.push(await scale(companies));
    if (out !== undefined) writeFileSync(out, JSON.stringify(results, null, 2) + "\n");
  }
}
