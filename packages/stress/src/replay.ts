// A benchmark scenario twice: once by `spool bench`, in one process and one
// engine, and once over HTTP by a server that is sent the same rows in the
// same runs. The two stores have to agree on who is who.
//
//   bun src/replay.ts ../../benchmarks/resolution/voters/5x500k.toml
//   bun src/replay.ts --out results.json ../../benchmarks/resolution/music/2m.toml
//
// `spool bench` downloads the scenario's public data, scores the result
// against the truth, and leaves a store. A server opens that store and is
// read from end to end, which gives the rows of each ingest run and who
// each record was placed with. A second server with a new store is then
// sent those rows and asked to match where the first run matched. It is
// read from end to end too, its calls are checked against each other, and
// both stores are verified.
//
// Only ingests and matches by the rules are replayed. A scenario with
// corrections, documents, a strategy of its own or rows that are taken back
// is refused.

import { cpSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import type { SpoolClient } from "../../client/src/index.ts";
import type { RunEntry } from "../../client/src/types.ts";

import { finished } from "./scale.ts";
import { binary, scratch, serve, storeSize, timed, verify } from "./server.ts";
import { faults } from "./state.ts";
import type { State } from "./state.ts";

/** The most rows one call of `POST /v1/ingest` carries. */
const CHUNK = 100_000;
const PAGE = 1000;

type Row = Record<string, unknown>;

export interface ReplayStep {
  step: string;
  seconds: number;
  memory_mb: number;
  store_mb: number;
  detail?: string;
}

/** The last step of what `spool bench` reported, as far as it is used here. */
export interface Quality {
  records: number;
  true_entities: number;
  identities: number;
  unassigned: number;
  precision: number | null;
  recall: number | null;
  f1: number | null;
  false_merges: number;
  false_splits: number;
}

export interface ReplayResult {
  scenario: string;
  records: number;
  identities: number;
  quality: Quality;
  steps: ReplayStep[];
  /** Where the two stores disagree on who is who. Empty when they agree. */
  differences: string[];
  /** Where the server's calls disagree with each other. */
  faults: string[];
  verified: string;
  peak_memory_mb: number;
}

/** What one end-to-end read of a server gives. */
interface Everything {
  state: State;
  /** The name each record has in both stores, by record ID. */
  names: Map<string, string>;
  /** The rows of each ingest run, in the order they are listed. */
  rows: Map<number, { source: string; entity_type: string; rows: Row[] }>;
}

/**
 * Reads every record and identity. A record is named by its source and
 * key, and by which version of that row it is, so that two stores that gave
 * it different IDs can be compared.
 */
async function everything(client: SpoolClient, keepRows: boolean): Promise<Everything> {
  const placed = new Map<string, string | null>();
  const names = new Map<string, string>();
  const versions = new Map<string, number>();
  const rows: Everything["rows"] = new Map();
  for (let after: string | undefined; ;) {
    const page = await client.records({ after, limit: PAGE });
    for (const record of page.records) {
      placed.set(record.id, record.identity ?? null);
      const row = `${record.source} ${record.key ?? JSON.stringify(record.payload)}`;
      const version = (versions.get(row) ?? 0) + 1;
      versions.set(row, version);
      names.set(record.id, version === 1 ? row : `${row} (version ${version})`);
      if (keepRows) {
        const run = rows.get(record.ingest_run) ?? { source: record.source, entity_type: record.entity_type, rows: [] };
        run.rows.push(record.payload as Row);
        rows.set(record.ingest_run, run);
      }
    }
    if (page.next == null) break;
    after = page.next;
  }
  const identities = new Map<string, number>();
  for (let after: string | undefined; ;) {
    const page = await client.identities({ after, limit: PAGE });
    for (const identity of page.identities) identities.set(identity.id, identity.members);
    if (page.next == null) break;
    after = page.next;
  }
  return { state: { head: (await client.info()).store.head, placed, identities }, names, rows };
}

/**
 * Who is with whom, by name: each record's name against the smallest name
 * in its identity. A record without an identity stands for itself, marked
 * as alone.
 */
function partition({ state, names }: Everything): Map<string, string> {
  const least = new Map<string, string>();
  for (const [record, identity] of state.placed) {
    if (identity === null) continue;
    const name = names.get(record)!;
    const known = least.get(identity);
    if (known === undefined || name < known) least.set(identity, name);
  }
  const with_ = new Map<string, string>();
  for (const [record, identity] of state.placed) {
    with_.set(names.get(record)!, identity === null ? "alone" : least.get(identity)!);
  }
  return with_;
}

/** Where two partitions of the same records differ, at most `limit` lines. */
function compare(reference: Map<string, string>, replayed: Map<string, string>, limit = 20): string[] {
  const found: string[] = [];
  let more = 0;
  const say = (line: string) => (found.length < limit ? found.push(line) : more++);
  for (const [name, group] of reference) {
    const other = replayed.get(name);
    if (other === undefined) say(`${name} is in the store of spool bench, and not in the server's`);
    else if (other !== group) say(`${name} is with ${group} in the store of spool bench, and with ${other} in the server's`);
  }
  for (const name of replayed.keys()) {
    if (!reference.has(name)) say(`${name} is in the server's store, and not in that of spool bench`);
  }
  if (more > 0) found.push(`and ${more} more`);
  return found;
}

function quality(report: string): Quality {
  const steps = JSON.parse(readFileSync(report, "utf8")).steps as Array<{ metrics: Record<string, any> }>;
  const metrics = steps.at(-1)!.metrics;
  return {
    records: metrics.records,
    true_entities: metrics.true_entities,
    identities: metrics.identities,
    unassigned: metrics.unassigned,
    precision: metrics.pairs.precision,
    recall: metrics.pairs.recall,
    f1: metrics.pairs.f1,
    false_merges: metrics.false_merges,
    false_splits: metrics.false_splits,
  };
}

/** The project directory a scenario names, which is `.` unless it says. */
function projectOf(scenario: string): string {
  const named = /^project\s*=\s*"([^"]*)"/m.exec(readFileSync(scenario, "utf8"))?.[1] ?? ".";
  return resolve(dirname(scenario), named);
}

export async function replay(
  scenario: string,
  seed = 1,
  log: (line: string) => void = console.log,
): Promise<ReplayResult> {
  scenario = resolve(scenario);
  const dir = scratch("replay");
  const steps: ReplayStep[] = [];
  let peak = 0;
  const record = (step: string, seconds: number, memory: number, store: string, detail?: string) => {
    peak = Math.max(peak, memory);
    const line: ReplayStep = {
      step,
      seconds: Math.round(seconds * 100) / 100,
      memory_mb: memory,
      store_mb: storeSize(store),
      ...(detail === undefined ? {} : { detail }),
    };
    steps.push(line);
    log(
      `  ${step.padEnd(28)} ${line.seconds.toFixed(2).padStart(9)} s  ${String(memory).padStart(6)} MB  ${String(
        line.store_mb,
      ).padStart(6)} MB  ${detail ?? ""}`,
    );
  };

  // The server builds the project where it is, so it is given a copy.
  const project = join(dir, "project");
  cpSync(projectOf(scenario), project, { recursive: true, filter: (path) => !path.endsWith("/.spool") });
  const referenceStore = join(dir, "reference.duckdb");
  const report = join(dir, "reference.json");

  try {
    log(`${scenario}, in ${dir}`);
    const [, benchSeconds] = await timed(async () => {
      const bench = Bun.spawn(
        [binary(), "bench", scenario, "--db", referenceStore, "--out", report, "--list", "0", "--seed", String(seed)],
        { stdout: "ignore", stderr: "pipe" },
      );
      const complaints = await new Response(bench.stderr).text();
      if ((await bench.exited) !== 0) throw new Error(`spool bench failed:\n${complaints}`);
    });
    const measured = quality(report);
    record(
      "spool bench",
      benchSeconds,
      0,
      referenceStore,
      `${measured.records} records, ${measured.identities} identities, precision ${measured.precision?.toFixed(4)}, recall ${measured.recall?.toFixed(4)}`,
    );

    // What `spool bench` did, read back over HTTP.
    const first = await serve({ project, store: referenceStore, seed });
    let reference: Everything;
    let runs: RunEntry[];
    try {
      const [read, seconds] = await timed(() => everything(first.client, true));
      reference = read;
      runs = (await first.client.runs({ limit: PAGE })).sort((a, b) => a.run - b.run);
      record("read the store of spool bench", seconds, first.memory(), referenceStore, `${read.state.placed.size} records`);
      if ((await first.stop()) !== 0) throw new Error(`the server stopped badly:\n${first.complaints()}`);
    } finally {
      if (first.alive()) await first.kill();
    }
    for (const run of runs) {
      const replayable =
        run.kind === "reconcile" ||
        (run.kind === "ingest" && run.extractor == null) ||
        (run.kind === "match" && (run.strategy == null || run.strategy.name === "rules"));
      if (!replayable || run.status !== "succeeded") {
        throw new Error(`run ${run.run} is a ${run.kind} run that ${run.status}, which is not replayed`);
      }
    }
    if (reference.state.head !== runs.filter((run) => run.kind === "match").length) {
      throw new Error("the scenario changes identities other than by matching, which is not replayed");
    }
    const referenceGroups = partition(reference);
    const expected = { records: reference.state.placed.size, identities: reference.state.identities.size };
    reference.state.placed.clear();
    reference.names.clear();

    // The same runs, over HTTP, in a new store.
    const store = join(dir, "store.duckdb");
    const server = await serve({ project, store, seed, build: false });
    const watch = setInterval(() => (peak = Math.max(peak, server.memory())), 250);
    try {
      const { client } = server;
      for (const run of runs) {
        if (run.kind === "ingest") {
          const sent = reference.rows.get(run.run);
          if (sent === undefined) continue;
          reference.rows.delete(run.run);
          const [, seconds] = await timed(async () => {
            for (let from = 0; from < sent.rows.length; from += CHUNK) {
              const ingest = await finished(
                client,
                await client.ingest({
                  source: sent.source,
                  entity_type: sent.entity_type,
                  rows: sent.rows.slice(from, from + CHUNK),
                }),
              );
              if (ingest?.status !== "succeeded") throw new Error(`ingest ${ingest?.status}: ${ingest?.error ?? ""}`);
            }
          });
          record(`ingest ${sent.source}`, seconds, server.memory(), store, `${sent.rows.length} rows`);
        } else if (run.kind === "match") {
          const [match, seconds] = await timed(async () =>
            finished(client, await client.startRun({ entity_type: run.entity_type! })),
          );
          if (match !== null && match.status !== "succeeded") {
            throw new Error(`the match run ${match.status}: ${match.error ?? ""}`);
          }
          const resolution = match?.resolution;
          record(
            `match ${run.entity_type}`,
            seconds,
            server.memory(),
            store,
            resolution == null
              ? "nothing to match"
              : `${resolution.candidates} candidates, ${resolution.identities_created} identities created, ${resolution.records_assigned} assigned`,
          );
        }
      }

      const [replayed, seconds] = await timed(() => everything(client, false));
      record("read the server's store", seconds, server.memory(), store, `${replayed.state.placed.size} records`);
      const differences = compare(referenceGroups, partition(replayed));
      const found = await faults(client, replayed.state);

      clearInterval(watch);
      const [code, stopSeconds] = await timed(() => server.stop());
      if (code !== 0) throw new Error(`the server stopped with code ${code}:\n${server.complaints()}`);
      log(`  stopped in ${stopSeconds.toFixed(2)} s`);
      const verified = verify(store);
      log(`  ${verified}`);
      verify(referenceStore);
      for (const line of [...differences, ...found]) log(`  FAULT ${line}`);
      return {
        scenario,
        ...expected,
        quality: measured,
        steps,
        differences,
        faults: found,
        verified,
        peak_memory_mb: peak,
      };
    } finally {
      clearInterval(watch);
      if (server.alive()) await server.kill();
    }
  } finally {
    if (process.env.SPOOL_STRESS_KEEP === undefined) rmSync(dir, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const out = args.includes("--out") ? args[args.indexOf("--out") + 1] : undefined;
  const scenarios = args.filter((arg, index) => !arg.startsWith("--") && args[index - 1] !== "--out");
  if (scenarios.length === 0) throw new Error("name a scenario file, as in `bun src/replay.ts ../../benchmarks/resolution/music/20k.toml`");
  const results: ReplayResult[] = [];
  let failed = false;
  for (const scenario of scenarios) {
    const result = await replay(scenario);
    results.push(result);
    failed ||= result.differences.length > 0 || result.faults.length > 0;
    if (out !== undefined) writeFileSync(out, JSON.stringify(results, null, 2) + "\n");
  }
  if (failed) process.exit(1);
}
