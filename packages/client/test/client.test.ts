// The client against the real server.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { readFileSync, rmSync } from "node:fs";
import { join } from "node:path";

import { SpoolClient, SpoolError } from "../src/index.ts";
import type { Changeset, RunRecord } from "../src/index.ts";
import { TOKEN, companies, copyProject, readCsv, serve, spool, temporaryDirectory, writeThings } from "./server.ts";
import type { Server } from "./server.ts";

const cleanup: string[] = [];
const servers: Server[] = [];

afterAll(async () => {
  for (const server of servers) await server.stop();
  for (const dir of cleanup) rmSync(dir, { recursive: true, force: true });
});

async function thingsServer(seed: number): Promise<Server> {
  const project = temporaryDirectory();
  cleanup.push(project);
  writeThings(project);
  const server = await serve(project, seed);
  servers.push(server);
  return server;
}

async function ingestThings(client: SpoolClient, rows: Array<Record<string, string>>): Promise<RunRecord> {
  const run = await client.ingest({ source: "shop", entity_type: "thing", key: "id", rows });
  expect(run.status).toBe("succeeded");
  return run;
}

const twoAnvilsAndAHammer = [
  { id: "1", code: "anvil" },
  { id: "2", code: "anvil" },
  { id: "3", code: "hammer" },
];

async function failureOf(call: Promise<unknown>): Promise<SpoolError> {
  try {
    await call;
  } catch (error) {
    expect(error).toBeInstanceOf(SpoolError);
    return error as SpoolError;
  }
  throw new Error("the call succeeded");
}

describe("the company scenario", () => {
  // The files of the scenario, in the order it ingests them, in two steps.
  const steps = [
    [
      { file: "batch-1-crm.csv", source: "crm", key: "id" },
      { file: "batch-1-registry.csv", source: "registry", key: "number" },
    ],
    [
      { file: "batch-2-crm.csv", source: "crm", key: "id" },
      { file: "batch-2-registry.csv", source: "registry", key: "number" },
    ],
  ];
  const seed = 21;
  let server: Server;

  beforeAll(async () => {
    const project = copyProject(companies);
    cleanup.push(project);
    server = await serve(project, seed);
    servers.push(server);
    for (const step of steps) {
      for (const { file, source, key } of step) {
        const rows = readCsv(join(companies, file));
        const run = await server.client.ingest({ source, entity_type: "company", key, rows });
        expect(run.status).toBe("succeeded");
        expect(run.ingest?.received).toBe(rows.length);
      }
      const run = await server.client.startRun({ entity_type: "company" });
      expect(run?.status).toBe("succeeded");
    }
  });

  test("the same seed gives the same changesets as the library", async () => {
    // `spool bench` runs the scenario in one process, with the library.
    const dir = temporaryDirectory();
    cleanup.push(dir);
    const out = join(dir, "results.json");
    spool("bench", companies, "--seed", String(seed), "--db", join(dir, "bench.duckdb"), "--out", out);
    const report = JSON.parse(readFileSync(out, "utf8")) as {
      steps: Array<{ resolve: { resolved: Array<{ committed: { content_hash: string } }> } }>;
    };
    const expected = report.steps.flatMap((step) => step.resolve.resolved.map((type) => type.committed.content_hash));
    expect(expected).toHaveLength(2);

    const log = await server.client.changesets();
    expect(log.head).toBe(2);
    expect(log.changesets.map((entry) => entry.content_hash)).toEqual(expected);
  });

  test("an observation is found by its key and leads to its identity", async () => {
    const { client } = server;
    const found = await client.observations({ source: "crm", key: "c-101" });
    expect(found.observations).toHaveLength(1);
    const observation = found.observations[0]!;
    expect(observation.payload).toMatchObject({ id: "c-101" });
    expect(await client.observation(observation.id)).toEqual(observation);

    const identity = await client.identity(observation.identity!);
    expect(identity.current).toBe(identity.id);
    expect(identity.members.map((member) => member.observation)).toContain(observation.id);
    expect(identity.members.length).toBeGreaterThan(1);

    const history = await client.identityHistory(identity.id);
    expect(history[0]!.seq).toBe(1);
    expect(history[0]!.ops[0]).toMatchObject({ op: "create_identity", identity: identity.id });

    const unknown = "00000000-0000-7000-8000-000000000000";
    expect(await client.identitiesOf([observation.id, unknown])).toEqual([identity.id, null]);
    expect((await failureOf(client.identity(unknown))).code).toBe("not_found");
  });

  test("identities are listed in pages", async () => {
    const { client } = server;
    const info = await client.info();
    expect(info.api).toBe("v1");
    const total = info.store.entity_types[0]!.identities;

    const listed: string[] = [];
    let after: string | undefined;
    do {
      const page = await client.identities({ entity_type: "company", limit: 5, after });
      expect(page.identities.length).toBeLessThanOrEqual(5);
      listed.push(...page.identities.map((identity) => identity.id));
      after = page.next ?? undefined;
    } while (after !== undefined);
    expect(listed).toHaveLength(total);
    expect(new Set(listed).size).toBe(total);
  });

  test("the record that fits two identities is a typed review item", async () => {
    const { client } = server;
    const items = await client.reviewItems({ status: "open" });
    expect(items).toHaveLength(1);
    const item = items[0]!;
    expect(item.kind).toBe("possible_merge");
    expect(item.entity_type).toBe("company");
    expect(item.identities).toHaveLength(2);

    const stuck = await client.observations({ unassigned: true });
    expect(stuck.observations.map((observation) => observation.id)).toEqual(item.observations);
    expect(stuck.observations[0]!.key).toBe("c-117");
    expect(stuck.observations[0]!.identity).toBeNull();
  });
});

describe("drafts", () => {
  test("a draft left for review is listed, read and approved", async () => {
    const { client } = await thingsServer(1);
    await ingestThings(client, twoAnvilsAndAHammer);

    const run = await client.startRun({ entity_type: "thing", commit: false });
    expect(run?.status).toBe("drafted");
    const id = run!.resolution!.draft!;
    expect((await client.changesets()).head).toBe(0);

    const open = await client.drafts({ status: "open" });
    expect(open.map((draft) => draft.id)).toEqual([id]);
    const record = await client.draft(id);
    expect(record.draft.identities_created).toBe(2);
    expect(record.changeset.ops).toHaveLength(5);

    const committed = await client.approveDraft(id);
    expect(committed).toMatchObject({ seq: 1, changeset: id });
    expect((await client.run(run!.run)).status).toBe("succeeded");
    expect(await client.drafts({ status: "open" })).toEqual([]);
    expect((await failureOf(client.approveDraft(id))).code).toBe("conflict");
  });

  test("a rejected draft does not strand its observations", async () => {
    const { client } = await thingsServer(2);
    await ingestThings(client, twoAnvilsAndAHammer);

    const first = await client.startRun({ entity_type: "thing", commit: false });
    const rejected = await client.rejectDraft(first!.resolution!.draft!, "not yet");
    expect(rejected).toMatchObject({ status: "rejected", rejected_reason: "not yet" });
    expect((await client.observations({ unassigned: true })).observations).toHaveLength(3);

    const second = await client.startRun({ entity_type: "thing" });
    expect(second?.status).toBe("succeeded");
    expect(second?.resolution?.observations_assigned).toBe(3);
    expect((await client.observations({ unassigned: true })).observations).toEqual([]);
    expect(await client.startRun({ entity_type: "thing" })).toBeNull();
  });

  test("approving a draft that went stale names the operations at fault", async () => {
    const { client } = await thingsServer(3);
    await ingestThings(client, twoAnvilsAndAHammer);
    const run = await client.startRun({ entity_type: "thing", commit: false });
    const { changeset } = await client.draft(run!.resolution!.draft!);

    // Someone commits the same assignments first, in a changeset of their own.
    const created = changeset.ops.flatMap((op) => (op.op === "create_identity" ? [op.identity] : []));
    const minted = await client.mintIds({ identities: created.length, changesets: 1 });
    let theirs = JSON.stringify({ ...changeset, id: minted.changesets[0], actor: "user:someone", run: null });
    created.forEach((identity, index) => {
      theirs = theirs.replaceAll(identity, minted.identities[index]!);
    });
    expect((await client.commit(JSON.parse(theirs) as Changeset)).seq).toBe(1);

    const failure = await failureOf(client.approveDraft(changeset.id));
    expect(failure.status).toBe(409);
    expect(failure.code).toBe("rejected");
    expect(failure.rejections.length).toBeGreaterThan(0);
    for (const rejection of failure.rejections) {
      expect(rejection.op).toEqual(changeset.ops[rejection.op_index!]!);
    }
    expect((await client.draft(changeset.id)).draft.status).toBe("open");
  });
});

describe("a reviewer", () => {
  const actor = "user:reviewer";

  async function thing(client: SpoolClient, key: string) {
    const { observations } = await client.observations({ source: "shop", key });
    expect(observations).toHaveLength(1);
    return observations[0]!;
  }

  async function keys(client: SpoolClient, identity: string): Promise<string[]> {
    const view = await client.identity(identity);
    return view.members.map((member) => member.key!).sort();
  }

  test("merges two identities and takes the merge back", async () => {
    const { client } = await thingsServer(9);
    await ingestThings(client, [
      { id: "1", code: "anvil" },
      { id: "2", code: "hammer" },
    ]);
    await client.startRun({ entity_type: "thing" });
    const anvil = (await thing(client, "1")).identity!;
    const hammer = (await thing(client, "2")).identity!;

    // No ID and no base: the server mints one and takes the head of the log.
    const merged = await client.commit({
      actor,
      reason: "one tool",
      ops: [{ op: "merge", from: anvil, into: hammer }],
    });
    expect(merged.seq).toBe(2);
    // The merged ID still answers, with the identity that stands for it.
    expect(await client.identity(anvil)).toMatchObject({ id: anvil, current: hammer });
    expect(await keys(client, anvil)).toEqual(["1", "2"]);

    const preview = await client.undo(merged.seq, { actor, commit: false });
    expect(preview.committed ?? null).toBeNull();
    expect(preview.changeset.ops.map((op) => op.op)).toEqual(["reinstate", "assign"]);
    expect((await client.changesets()).head).toBe(2);

    const undone = await client.undo(merged.seq, { actor });
    expect(undone.committed?.seq).toBe(3);
    expect(await client.identity(anvil)).toMatchObject({ id: anvil, current: anvil });
    expect(await keys(client, anvil)).toEqual(["1"]);
    expect(await keys(client, hammer)).toEqual(["2"]);

    expect((await failureOf(client.undo(99, { actor }))).code).toBe("not_found");
    // A merge written before the undo is stale, and says so.
    const stale = await failureOf(
      client.commit({ actor, base: 1, ops: [{ op: "merge", from: anvil, into: hammer }] }),
    );
    expect(stale.code).toBe("rejected");
    expect(stale.rejections[0]).toMatchObject({ op_index: 0, reason: { reason: "stale_merge" } });
  });

  test("splits an identity, and the constraint keeps the sides apart", async () => {
    const { client } = await thingsServer(10);
    await ingestThings(client, [
      { id: "1", code: "anvil" },
      { id: "2", code: "anvil" },
      { id: "3", code: "anvil" },
    ]);
    await client.startRun({ entity_type: "thing" });
    const third = await thing(client, "3");
    const anvils = third.identity!;

    const split = await client.split(anvils, { observations: [third.id], actor, reason: "another anvil" });
    expect(split).toMatchObject({ kept: anvils, moved: [third.id] });
    expect(await keys(client, anvils)).toEqual(["1", "2"]);
    expect(await keys(client, split.identity)).toEqual(["3"]);

    const constraints = await client.constraints();
    expect(constraints).toHaveLength(1);
    expect(constraints[0]).toMatchObject({ kind: "cannot_link", from: split.committed.seq });
    expect([constraints[0]!.a, constraints[0]!.b]).toContain(third.id);
    expect(await client.constraints({ observation: third.id })).toEqual(constraints);
    expect(await client.constraints({ identity: split.identity })).toEqual(constraints);

    // Merging the sides again would break the constraint.
    const merge = await failureOf(
      client.commit({ actor, ops: [{ op: "merge", from: split.identity, into: anvils }] }),
    );
    expect(merge.rejections[0]).toMatchObject({ reason: { reason: "constraint_violated" } });

    // A fourth anvil matches both sides, and is left for the reviewer, who
    // closes the item without placing it.
    await ingestThings(client, [{ id: "4", code: "anvil" }]);
    await client.startRun({ entity_type: "thing" });
    const [item] = await client.reviewItems({ status: "open" });
    expect(item!.kind).toBe("possible_merge");
    expect(await client.reviewItem(item!.id)).toEqual(item!);

    const dismissed = await client.dismissReview(item!.id, { actor, reason: "not ours" });
    expect(dismissed).toMatchObject({ status: "dismissed", dismissed: { by: actor, reason: "not ours" } });
    expect(await client.reviewItems({ status: "open" })).toEqual([]);
    expect(await client.startRun({ entity_type: "thing" })).toBeNull();
    expect((await thing(client, "4")).identity ?? null).toBeNull();
    expect((await failureOf(client.dismissReview(item!.id, { actor }))).code).toBe("conflict");
    expect((await failureOf(client.reviewItem(999))).code).toBe("not_found");

    // Removing the constraint leaves it in the history.
    const { a, b } = constraints[0]!;
    const removed = await client.commit({ actor, ops: [{ op: "unconstrain", kind: "cannot_link", a, b }] });
    expect(await client.constraints()).toEqual([]);
    expect(await client.constraints({ removed: true })).toMatchObject([{ a, b, to: removed.seq }]);
  });

  test("a row that changed supersedes the one before", async () => {
    const { client } = await thingsServer(11);
    await ingestThings(client, [{ id: "1", code: "anvil" }]);
    await client.startRun({ entity_type: "thing" });
    const first = await thing(client, "1");

    const run = await ingestThings(client, [{ id: "1", code: "anvil", weight: "50kg" }]);
    expect(run.ingest?.superseded).toBe(1);
    await client.startRun({ entity_type: "thing" });

    const { observations } = await client.observations({ source: "shop", key: "1" });
    expect(observations).toHaveLength(2);
    const second = observations.find((observation) => observation.id !== first.id)!;
    expect(second.supersedes).toBe(first.id);
    expect((await client.observation(first.id)).superseded_by).toBe(second.id);
    expect(second.identity).toBe(first.identity!);
  });
});

describe("a run judged by the caller", () => {
  test("candidates are read, judged and reconciled", async () => {
    const { client } = await thingsServer(4);
    await ingestThings(client, [...twoAnvilsAndAHammer, { id: "4", code: "hammer" }]);

    const run = await client.startRun({ entity_type: "thing", strategy: { name: "by-hand", version: "1" } });
    expect(run?.status).toBe("open");
    const id = run!.run;

    const candidates = await client.candidates(id);
    expect(candidates.map((candidate) => candidate.left_code).sort()).toEqual(["anvil", "hammer"]);
    // The same pairs come as an Arrow stream, which starts with a continuation marker.
    const arrow = await client.candidatesArrow(id);
    expect([...arrow.slice(0, 4)]).toEqual([0xff, 0xff, 0xff, 0xff]);

    const stored = await client.submitJudgements(
      id,
      candidates.map((candidate) => ({
        left_id: candidate.left_id as string,
        right_id: candidate.right_id as string,
        verdict: candidate.left_code === "anvil" ? "match" : "no_match",
        score: 0.5,
        detail: { by: "hand" },
      })),
    );
    expect(stored).toBe(2);

    const resolution = await client.reconcile(id);
    expect(resolution.judgements).toEqual({ match: 1, no_match: 1, unsure: 0 });
    expect(resolution.identities_created).toBe(3);
    expect(resolution.committed?.seq).toBe(1);
    expect((await client.run(id)).status).toBe("succeeded");

    const anvils = candidates.find((candidate) => candidate.left_code === "anvil")!;
    const [left, right] = await client.identitiesOf([anvils.left_id as string, anvils.right_id as string]);
    expect(left).not.toBeNull();
    expect(left).toBe(right);
    expect((await failureOf(client.submitJudgements(id, []))).code).toBe("conflict");
  });

  test("an open run can be cancelled", async () => {
    const { client } = await thingsServer(5);
    await ingestThings(client, twoAnvilsAndAHammer);
    const run = await client.startRun({ entity_type: "thing", strategy: { name: "by-hand" } });
    const cancelled = await client.cancelRun(run!.run);
    expect(cancelled).toMatchObject({ status: "failed", error: "cancelled" });
    expect((await client.runs({ kind: "match" })).map((record) => record.status)).toEqual(["failed"]);
  });
});

describe("calls that take a while", () => {
  test("a run that outlasts the wait is followed until it ends", async () => {
    const { client } = await thingsServer(6);
    const rows = Array.from({ length: 30_000 }, (_, index) => ({ id: String(index), code: String(index) }));

    const started = await client.ingest({ source: "bulk", entity_type: "thing", key: "id", rows, wait_ms: 0 });
    expect(started.status).toBe("running");
    const finished = await client.waitForRun(started.run);
    expect(finished.status).toBe("succeeded");
    expect(finished.ingest?.inserted).toBe(30_000);

    const resolving = await client.startRun({ entity_type: "thing", wait_ms: 0 });
    expect(resolving?.status).toBe("running");
    // A wait that is too short hands back the run as it is.
    expect((await client.waitForRun(resolving!.run, { timeoutMs: 0 })).status).toBe("running");
    const resolved = await client.waitForRun(resolving!.run, { timeoutMs: 120_000 });
    expect(resolved.status).toBe("succeeded");
    expect(resolved.resolution?.identities_created).toBe(30_000);
  });
});

describe("failures", () => {
  test("a wrong token is refused", async () => {
    const { url, client } = await thingsServer(7);
    const stranger = new SpoolClient({ url, token: `${TOKEN}-but-wrong` });
    const failure = await failureOf(stranger.info());
    expect(failure.status).toBe(401);
    expect(failure.code).toBe("unauthorized");
    expect(await stranger.healthy()).toBe(true);
    expect((await client.info()).store.head).toBe(0);
    // A trailing slash on the URL is not part of the path.
    expect((await new SpoolClient({ url: `${url}/`, token: TOKEN }).info()).api).toBe("v1");
  });

  test("what the server will not take is described", async () => {
    const { client } = await thingsServer(8);
    const undeclared = await failureOf(client.startRun({ entity_type: "gadget" }));
    expect(undeclared).toMatchObject({ status: 400, code: "invalid" });
    expect(undeclared.message).toContain("`gadget` is not declared");
    expect((await failureOf(client.run(999))).code).toBe("not_found");

    const minted = await client.mintIds({ identities: 1, changesets: 1 });
    const rejected = await failureOf(
      client.commit({
        id: minted.changesets[0]!,
        base: 0,
        actor: "user:test",
        ops: [
          { op: "create_identity", identity: minted.identities[0]!, entity_type: "thing" },
          { op: "assign", observation: "00000000-0000-7000-8000-000000000002", to: minted.identities[0]! },
        ],
      }),
    );
    expect(rejected.code).toBe("rejected");
    expect(rejected.rejections).toHaveLength(1);
    expect(rejected.rejections[0]).toMatchObject({ op_index: 1, reason: { reason: "unknown_observation" } });
  });

  test("no server at all is not healthy", async () => {
    const nowhere = new SpoolClient({ url: "http://127.0.0.1:1", token: TOKEN });
    expect(await nowhere.healthy()).toBe(false);
  });
});
