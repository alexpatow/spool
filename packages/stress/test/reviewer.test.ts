// A small run of `src/reviewer.ts` with one reviewer, so that the stress
// test itself keeps working and a fault it once found does not come back.
// The big runs are started by hand: see the head of that file.

import { expect, test } from "bun:test";

import { reviewer } from "../src/reviewer.ts";

const MINUTES = 60_000;

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
