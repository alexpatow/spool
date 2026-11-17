// Small runs of each stress test, so that the tests themselves keep working
// and a fault they once found does not come back. The big runs are started
// by hand: see the head of each file in `src`.

import { expect, test } from "bun:test";

import { rmSync } from "node:fs";
import { join } from "node:path";

import { crash, steps } from "../src/crash.ts";
import { generate, records } from "../src/generate.ts";
import { hostile } from "../src/hostile.ts";
import { reviewer } from "../src/reviewer.ts";
import { scale } from "../src/scale.ts";
import { scratch, serve } from "../src/server.ts";
import { writeProject } from "../src/write.ts";

const MINUTES = 60_000;

test("the same seed generates the same records, and the truth names each of them", () => {
  const once = generate({ companies: 300, seed: 7, batches: 3 });
  expect(generate({ companies: 300, seed: 7, batches: 3 })).toEqual(once);
  expect(generate({ companies: 300, seed: 8, batches: 3 })).not.toEqual(once);

  expect(once.batches).toHaveLength(3);
  const named = new Set(once.truth.map((line) => `${line.source}:${line.key}`));
  for (const batch of once.batches) {
    for (const row of batch.crm) expect(named.has(`crm:${row.id}`)).toBe(true);
    for (const row of batch.registry) expect(named.has(`registry:${row.number}`)).toBe(true);
  }
  expect(records(once)).toBeGreaterThan(600);
});

test(
  "a store that grows in batches is matched, read and verified",
  async () => {
    const result = await scale(400, 1, 2, () => {});
    expect(result.verified).toStartWith("verified ");
    const last = result.steps.at(-1)!;
    expect(last.step).toBe("rematch");
    // A record that was sent again unchanged is not a new record.
    expect(last.records).toBeLessThanOrEqual(result.records);
    expect(last.records).toBeGreaterThan(result.records * 0.9);
    // Most records find their company: there are fewer identities than
    // records, and more than half as many as companies.
    expect(last.identities).toBeLessThan(result.records);
    expect(last.identities).toBeGreaterThan(200);
    expect(Object.keys(result.reads)).toContain("explain a pair");
  },
  5 * MINUTES,
);

test(
  "requests nobody should send are refused, and the server goes on working",
  async () => {
    const result = await hostile();
    expect(result.faults).toEqual([]);
    expect(result.cases).toBeGreaterThan(150);
  },
  5 * MINUTES,
);

test(
  "a server that is killed in the middle of its work loses nothing and ends where a calm one does",
  async () => {
    const result = await crash({ companies: 1500, kills: 2, seed: 4 });
    expect(result.faults).toEqual([]);
    expect(result.kills).toBe(2 * result.steps);
  },
  10 * MINUTES,
);

test(
  "a reviewer who does anything in any order never breaks the store",
  async () => {
    const result = await reviewer({ actions: 200, seed: 11 });
    expect(result.faults).toEqual([]);
    expect(result.actions).toBeGreaterThan(150);
    expect(result.restarts).toBeGreaterThan(0);
    expect(result.head).toBeGreaterThan(50);
  },
  10 * MINUTES,
);

test(
  "reviewers who work at the same time never break the store",
  async () => {
    const result = await reviewer({ actions: 200, seed: 12, reviewers: 6 });
    expect(result.faults).toEqual([]);
    expect(result.head).toBeGreaterThan(50);
  },
  10 * MINUTES,
);

test(
  "a step that was finished before the server was killed is finished again without a fault",
  async () => {
    // A set small enough to leave nothing for review, so that a second match has no work at all.
    const dir = scratch("twice");
    const generated = generate({ companies: 4, seed: 2, batches: 2 });
    writeProject(dir, generated);
    const server = await serve({ project: dir, store: join(dir, "store.duckdb") });
    try {
      for (const step of steps(generated)) {
        await step.finish(server.client);
        expect(await server.client.reviewItems({ status: "open" })).toEqual([]);
        await step.finish(server.client);
      }
    } finally {
      await server.kill();
      rmSync(dir, { recursive: true, force: true });
    }
  },
  5 * MINUTES,
);

test(
  "a set with a batch that brings nothing new is still loaded and verified",
  async () => {
    const result = await scale(2, 3, 3, () => {});
    expect(result.verified).toStartWith("verified ");
  },
  5 * MINUTES,
);
