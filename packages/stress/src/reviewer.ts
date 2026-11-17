// A reviewer who does anything the API allows, in an order chosen at random.
//
//   bun src/reviewer.ts [--actions 400] [--companies 150] [--seed 1] [--reviewers 1] [--quiet]
//
// Records arrive, are matched and are sent again changed, while identities
// are merged, split, taken apart and put back, constraints are set, earlier
// changesets are undone, drafts are approved and rejected, and the server is
// stopped or killed and started again. Some of what is sent is out of date
// or names things that do not exist, and has to be refused. The server may
// never answer with a failure of its own, the calls of the API have to keep
// agreeing with each other, and the store has to verify whenever it is
// closed.

import { rmSync } from "node:fs";
import { join } from "node:path";

import { SpoolError } from "../../client/src/index.ts";
import type { SpoolClient } from "../../client/src/index.ts";
import type { Op } from "../../client/src/types.ts";

import { generate, random } from "./generate.ts";
import type { Row } from "./generate.ts";
import { filled } from "./scale.ts";
import { scratch, serve, verify } from "./server.ts";
import type { Running } from "./server.ts";
import { faults, groups, read } from "./state.ts";
import type { State } from "./state.ts";
import { writeProject } from "./write.ts";

export interface ReviewerOptions {
  actions: number;
  companies?: number;
  seed?: number;
  /** After how many actions the calls of the API are compared. */
  every?: number;
  /**
   * How many reviewers work at the same time. With more than one, each acts
   * on what it read a moment ago, the server is not restarted, and the calls
   * are compared when all are done.
   */
  reviewers?: number;
  log?: (line: string) => void;
}

export interface ReviewerResult {
  actions: number;
  /** How often each action was accepted and refused. */
  outcomes: Record<string, { accepted: number; refused: number }>;
  restarts: number;
  head: number;
  /** Everything that was wrong, each with the actions before it. Empty when nothing was. */
  faults: string[];
}

const ACTOR = "user:stress";
const NOBODY = "00000000-0000-7000-8000-000000000000";

/** The statuses that say the caller asked for something that cannot be done. */
const REFUSALS = new Set([400, 404, 409]);

export async function reviewer({
  actions,
  companies = 150,
  seed = 1,
  every = 10,
  reviewers = 1,
  log = () => {},
}: ReviewerOptions): Promise<ReviewerResult> {
  const dir = scratch("reviewer");
  const generated = generate({ companies, seed, batches: 3 });
  writeProject(dir, generated);
  const store = join(dir, "store.duckdb");
  const next = random(seed);
  const below = (limit: number) => Math.floor(next() * limit);
  const pick = <T>(items: T[]): T | undefined => items[below(items.length)];

  // Rows wait in the order they would arrive in. `sent` is what the store
  // was last given for each key.
  const waiting: Array<{ source: string; key: string; row: Row }> = [];
  for (const batch of generated.batches) {
    for (const row of batch.crm) waiting.push({ source: "crm", key: "id", row });
    for (const row of batch.registry) waiting.push({ source: "registry", key: "number", row });
  }
  const sent: Array<{ source: string; key: string; row: Row }> = [];

  const result: ReviewerResult = { actions: 0, outcomes: {}, restarts: 0, head: 0, faults: [] };
  const history: string[] = [];
  const fault = (message: string) => {
    result.faults.push(`${message}\n    after: ${history.slice(-8).join("\n           ")}`);
  };
  let server: Running = await serve({ project: dir, store });
  let client: SpoolClient = server.client;

  const send = async (source: string, rows: Row[]) => {
    const run = await client.ingest({ source, entity_type: "company", rows: rows.map(filled) });
    if (run.status !== "succeeded") throw new Error(`the ingest ${run.status}: ${run.error ?? ""}`);
  };
  const match = async (request: { rematch?: boolean; commit?: boolean }) => {
    const run = await client.startRun({ entity_type: "company", ...request });
    const done = run?.status === "running" ? await client.waitForRun(run.run) : run;
    if (done !== null && done.status !== "succeeded" && done.status !== "drafted") {
      throw new Error(`the run ${done.status}: ${done.error ?? ""}`);
    }
    return done;
  };
  const members = (state: State, identity: string) =>
    [...state.placed].filter(([, placed]) => placed === identity).map(([record]) => record);
  const reopen = async (how: "stop" | "kill") => {
    const before = how === "stop" ? groups(await read(client)) : undefined;
    if (how === "stop") {
      const code = await server.stop();
      if (code !== 0) fault(`the server stopped with code ${code}: ${server.complaints()}`);
    } else {
      await server.kill();
    }
    try {
      verify(store);
    } catch (error) {
      fault(`after a ${how}: ${(error as Error).message}`);
    }
    server = await serve({ project: dir, store, build: false });
    client = server.client;
    result.restarts += 1;
    if (before !== undefined && groups(await read(client)).join("\n") !== before.join("\n")) {
      fault("the store says something else about who is who after it was closed and opened");
    }
  };

  // Each action is tried against what the store says now. An action that
  // has nothing to work on does something smaller instead of nothing.
  const menu: Array<[name: string, weight: number, act: (state: State) => Promise<void>]> = [
    [
      "ingest",
      10,
      async () => {
        const rows = waiting.splice(0, 1 + below(40));
        for (const source of ["crm", "registry"]) {
          const mine = rows.filter((row) => row.source === source);
          if (mine.length > 0)
            await send(
              source,
              mine.map((row) => row.row),
            );
        }
        sent.push(...rows);
      },
    ],
    [
      "send again, changed",
      6,
      async () => {
        const row = pick(sent);
        if (row === undefined) return;
        const field = row.source === "crm" ? "name" : "legal_name";
        if (below(3) > 0)
          row.row = { ...row.row, [field]: `${row.row[field] ?? ""} ${pick(["Group", "Ltd", "AB", ""])}` };
        await send(row.source, [row.row]);
      },
    ],
    ["match", 8, async () => void (await match({}))],
    ["match again", 3, async () => void (await match({ rematch: true }))],
    [
      "draft, then decide",
      4,
      async () => {
        const run = await match({ commit: false });
        const draft = run?.resolution?.draft;
        if (draft == null) return;
        if (below(3) === 0) await client.rejectDraft(draft, "stress");
        else if (below(2) === 0) await client.approveDraft(draft);
      },
    ],
    [
      "decide an open draft",
      2,
      async () => {
        const draft = pick(await client.drafts({ status: "open" }));
        if (draft === undefined) return;
        if (below(2) === 0) await client.approveDraft(draft.id);
        else await client.rejectDraft(draft.id);
      },
    ],
    [
      "merge",
      10,
      async (state) => {
        const ids = [...state.identities.keys()];
        const from = pick(ids);
        const into = pick(ids);
        if (from === undefined || into === undefined) return;
        await client.commit({ actor: ACTOR, base: state.head, ops: [{ op: "merge", from, into }] });
      },
    ],
    [
      "split",
      10,
      async (state) => {
        const identity = pick([...state.identities].filter(([, count]) => count > 1).map(([id]) => id));
        if (identity === undefined) return;
        const all = members(state, identity);
        const records = all.filter(() => below(2) === 0);
        await client.split(identity, {
          actor: ACTOR,
          records: records.length > 0 ? records : all.slice(0, 1),
          keep_apart: below(2) === 0,
          ...(below(2) === 0 ? { base: state.head } : {}),
        });
      },
    ],
    [
      "move a record",
      6,
      async (state) => {
        const [record, from] = pick([...state.placed]) ?? [];
        const to = pick([...state.identities.keys()]);
        if (record === undefined || to === undefined) return;
        await client.commit({ actor: ACTOR, ops: [{ op: "assign", record, from, to }] });
      },
    ],
    [
      "take a record out",
      3,
      async (state) => {
        const [record, from] = pick([...state.placed].filter(([, identity]) => identity !== null)) ?? [];
        if (record === undefined) return;
        await client.commit({ actor: ACTOR, ops: [{ op: "assign", record, from, to: null }] });
      },
    ],
    [
      "place a record in a new identity",
      4,
      async (state) => {
        const [record] = pick([...state.placed].filter(([, identity]) => identity === null)) ?? [];
        if (record === undefined) return;
        const identity = (await client.mintIds({ identities: 1 })).identities[0]!;
        await client.commit({
          actor: ACTOR,
          base: state.head,
          ops: [
            { op: "create_identity", identity, entity_type: "company" },
            { op: "assign", record, from: null, to: identity },
          ],
        });
      },
    ],
    [
      "constrain",
      8,
      async (state) => {
        const records = [...state.placed.keys()];
        const a = pick(records);
        const b = pick(records);
        if (a === undefined || b === undefined) return;
        const kind = below(2) === 0 ? "cannot_link" : "must_link";
        await client.commit({ actor: ACTOR, ops: [{ op: "constrain", a, b, kind }] });
      },
    ],
    [
      "undo",
      10,
      async (state) => {
        if (state.head === 0) return;
        // Mostly something recent, which is more often still undoable.
        const back = below(2) === 0 ? below(Math.min(5, state.head)) : below(state.head);
        await client.undo(state.head - back, { actor: ACTOR, commit: below(6) > 0 });
      },
    ],
    [
      "dismiss a review item",
      4,
      async () => {
        const item = pick(await client.reviewItems({ status: "open" }));
        if (item !== undefined) await client.dismissReview(item.id, { actor: ACTOR, reason: "stress" });
      },
    ],
    [
      "send something out of date",
      5,
      async (state) => {
        const ids = [...state.identities.keys()];
        const ops: Op[] = [{ op: "merge", from: pick(ids) ?? NOBODY, into: pick(ids) ?? NOBODY }];
        await client.commit({ actor: ACTOR, base: Math.max(0, state.head - 1 - below(4)), ops });
      },
    ],
    [
      "name what does not exist",
      4,
      async (state) => {
        const real = pick([...state.identities.keys()]) ?? NOBODY;
        const record = pick([...state.placed.keys()]) ?? NOBODY;
        const ops = pick<Op[]>([
          [{ op: "merge", from: NOBODY, into: real }],
          [{ op: "merge", from: real, into: real }],
          [{ op: "assign", record: NOBODY, from: null, to: real }],
          [{ op: "assign", record, from: NOBODY, to: real }],
          [{ op: "constrain", a: record, b: record, kind: "cannot_link" }],
          [{ op: "create_identity", identity: real, entity_type: "company" }],
          [],
        ])!;
        await client.commit({ actor: ACTOR, ops });
        throw new Error(`the server took a changeset it had to refuse: ${JSON.stringify(ops)}`);
      },
    ],
    ["stop and start", 2, async () => reopen("stop")],
    ["kill and start", 2, async () => reopen("kill")],
  ];
  const total = menu.reduce((sum, [, weight]) => sum + weight, 0);
  const choose = () => {
    let at = below(total);
    for (const entry of menu) {
      if (at < entry[1]) return entry;
      at -= entry[1];
    }
    return menu[0]!;
  };

  try {
    await send(
      "crm",
      waiting.splice(0, 30).map((row) => (sent.push(row), row.row)),
    );
    await match({});
    let taken = 0;
    const work = async () => {
      while (taken < actions && result.faults.length <= 20 && server.alive()) {
        const index = taken++;
        const [name, , act] = choose();
        if (reviewers > 1 && name.endsWith("and start")) continue;
        const state = await read(client);
        const outcome = (result.outcomes[name] ??= { accepted: 0, refused: 0 });
        let line = `${index}: ${name}`;
        try {
          await act(state);
          outcome.accepted += 1;
        } catch (error) {
          if (error instanceof SpoolError && REFUSALS.has(error.status)) {
            outcome.refused += 1;
            line += `, refused (${error.code}): ${error.message.slice(0, 160)}`;
          } else {
            history.push(line);
            const status = error instanceof SpoolError ? `${error.status} ${error.code}: ` : "";
            fault(`${name}: ${status}${(error as Error).message}`);
            if (!server.alive()) fault(`the server is gone: ${server.complaints()}`);
            continue;
          }
        }
        history.push(line);
        log(line);
        result.actions += 1;
        // With one reviewer nothing else writes, so the calls can be compared now.
        if (reviewers === 1 && (index + 1) % every === 0) {
          for (const found of await faults(client, await read(client))) fault(found);
        }
      }
    };
    await Promise.all(Array.from({ length: reviewers }, work));
    if (server.alive()) for (const found of await faults(client, await read(client))) fault(found);
    if (server.alive()) {
      result.head = (await client.info()).store.head;
      const code = await server.stop();
      if (code !== 0) fault(`the server stopped with code ${code}: ${server.complaints()}`);
      try {
        verify(store);
      } catch (error) {
        fault((error as Error).message);
      }
    }
    return result;
  } finally {
    if (server.alive()) await server.kill();
    if (process.env.SPOOL_STRESS_KEEP === undefined) rmSync(dir, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const option = (name: string, otherwise: number) =>
    args.includes(name) ? Number(args[args.indexOf(name) + 1]) : otherwise;
  const result = await reviewer({
    actions: option("--actions", 400),
    companies: option("--companies", 150),
    seed: option("--seed", 1),
    reviewers: option("--reviewers", 1),
    log: args.includes("--quiet") ? undefined : (line) => console.log(`  ${line}`),
  });
  console.log(`${result.actions} actions, ${result.restarts} restarts, the log ends at ${result.head}`);
  for (const [name, { accepted, refused }] of Object.entries(result.outcomes)) {
    console.log(`  ${name.padEnd(40)} ${String(accepted).padStart(5)} accepted ${String(refused).padStart(5)} refused`);
  }
  for (const found of result.faults) console.log(`FAULT ${found}`);
  process.exit(result.faults.length === 0 ? 0 : 1);
}
