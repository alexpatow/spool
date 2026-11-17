// What a server makes of being stopped at once, in the middle of its work.
//
//   bun src/crash.ts [--companies 20000] [--kills 3] [--seed 1]
//
// The same generated records are loaded twice. The first server is left
// alone. The second is killed at a moment chosen at random within each
// step, as often as `--kills` says, and started again on the same store;
// the step is then done again. After every kill the store has to verify,
// no run may be left `running`, and the calls of the API have to agree with
// each other. In the end both stores have to say the same about who is who.

import { rmSync } from "node:fs";
import { join } from "node:path";

import type { SpoolClient } from "../../client/src/index.ts";

import { generate, random } from "./generate.ts";
import type { Generated, Row } from "./generate.ts";
import { filled, finished, ingest } from "./scale.ts";
import { scratch, serve, timed, verify } from "./server.ts";
import type { Running } from "./server.ts";
import { faults, groups, read } from "./state.ts";
import { writeProject } from "./write.ts";

export interface CrashOptions {
  companies: number;
  seed?: number;
  /** How often the server is killed within each step. */
  kills?: number;
  log?: (line: string) => void;
}

export interface CrashResult {
  steps: number;
  kills: number;
  /** Kills that found a run in progress. */
  interrupted: number;
  /** Everything that was wrong. Empty when nothing was. */
  faults: string[];
}

interface Step {
  name: string;
  /** Starts the work and does not wait for it. */
  start(client: SpoolClient): void;
  /** Does the work and waits until it is done. */
  finish(client: SpoolClient): Promise<void>;
}

function steps(generated: Generated): Step[] {
  const load = (name: string, source: string, key: string, rows: Row[]): Step => ({
    name,
    start: (client) =>
      void client.ingest({ source, entity_type: "company", key, rows: rows.map(filled), wait_ms: 0 }).catch(() => {}),
    finish: async (client) => void (await ingest(client, source, key, rows)),
  });
  const match = (name: string, rematch: boolean): Step => ({
    name,
    start: (client) => void client.startRun({ entity_type: "company", rematch, wait_ms: 0 }).catch(() => {}),
    finish: async (client) => {
      const run = await finished(client, await client.startRun({ entity_type: "company", rematch }));
      if (run?.status !== "succeeded") throw new Error(`${name}: the run ${run?.status}: ${run?.error ?? ""}`);
    },
  });
  const all: Step[] = [];
  for (const [index, batch] of generated.batches.entries()) {
    all.push(load(`batch ${index + 1}: ingest crm`, "crm", "id", batch.crm));
    all.push(load(`batch ${index + 1}: ingest registry`, "registry", "number", batch.registry));
    all.push(match(`batch ${index + 1}: match`, false));
  }
  all.push(match("rematch", true));
  return all;
}

export async function crash({ companies, seed = 1, kills = 2, log = () => {} }: CrashOptions): Promise<CrashResult> {
  const dir = scratch(`crash-${companies}`);
  const generated = generate({ companies, seed, batches: 2 });
  writeProject(dir, generated);
  const work = steps(generated);
  const next = random(seed);
  const found: string[] = [];
  const result: CrashResult = { steps: work.length, kills: 0, interrupted: 0, faults: found };
  let server: Running | undefined;

  try {
    // The server that is left alone, which also says how long each step takes.
    // Neither server is given a seed: a seeded server mints the same IDs
    // again after a restart, which only a test can ask for.
    server = await serve({ project: dir, store: join(dir, "calm.duckdb") });
    const seconds: number[] = [];
    for (const step of work) seconds.push((await timed(() => step.finish(server!.client)))[1]);
    const calm = groups(await read(server.client));
    await server.stop();
    verify(server.store);

    const store = join(dir, "killed.duckdb");
    server = await serve({ project: dir, store, build: false });
    for (const [index, step] of work.entries()) {
      for (let kill = 0; kill < kills; kill++) {
        const wait = next() * seconds[index]! * 1200;
        step.start(server.client);
        await Bun.sleep(wait);
        await server.kill();
        result.kills += 1;
        try {
          verify(store);
        } catch (error) {
          found.push(`${step.name}, killed after ${wait.toFixed(0)} ms: ${(error as Error).message}`);
        }
        server = await serve({ project: dir, store, build: false });
        const runs = await server.client.runs({ limit: 1000 });
        const running = runs.filter((run) => run.status === "running");
        if (running.length > 0) {
          found.push(`${step.name}: run ${running.map((run) => run.run).join(", ")} is still running after a restart`);
        }
        const interrupted = runs.filter((run) => run.error?.startsWith("interrupted")).length;
        log(`${step.name}: killed after ${wait.toFixed(0)} ms, ${interrupted} runs interrupted so far`);
        result.interrupted = interrupted;
      }
      await step.finish(server.client);
      for (const fault of await faults(server.client, await read(server.client))) found.push(`${step.name}: ${fault}`);
    }

    const killed = groups(await read(server.client));
    const code = await server.stop();
    if (code !== 0) found.push(`the server stopped with code ${code}: ${server.complaints()}`);
    verify(store);
    if (killed.join("\n") !== calm.join("\n")) {
      const lost = calm.filter((line) => !killed.includes(line)).length;
      found.push(
        `the store that was killed says something else about who is who: ${lost} of ${calm.length} groups differ`,
      );
    }
    return result;
  } finally {
    if (server?.alive()) await server.kill();
    if (process.env.SPOOL_STRESS_KEEP === undefined) rmSync(dir, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const option = (name: string, otherwise: number) =>
    args.includes(name) ? Number(args[args.indexOf(name) + 1]) : otherwise;
  const result = await crash({
    companies: option("--companies", 20_000),
    seed: option("--seed", 1),
    kills: option("--kills", 3),
    log: (line) => console.log(`  ${line}`),
  });
  console.log(`${result.steps} steps, ${result.kills} kills, ${result.interrupted} runs interrupted`);
  for (const fault of result.faults) console.log(`FAULT ${fault}`);
  process.exit(result.faults.length === 0 ? 0 : 1);
}
