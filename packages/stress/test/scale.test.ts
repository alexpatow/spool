// Small runs of `src/scale.ts` and of the records `src/generate.ts` makes for
// it, so that the stress test itself keeps working and a fault it once found
// does not come back. The big runs are started by hand: see the head of
// `src/scale.ts`.

import { expect, test } from "bun:test";

import { generate, records } from "../src/generate.ts";
import { scale } from "../src/scale.ts";

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
  "a set with a batch that brings nothing new is still loaded and verified",
  async () => {
    const result = await scale(2, 3, 3, () => {});
    expect(result.verified).toStartWith("verified ");
  },
  5 * MINUTES,
);
