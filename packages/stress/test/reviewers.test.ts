// A small run of `src/reviewer.ts` with several reviewers at once, so that
// the stress test itself keeps working and a fault it once found does not
// come back. The big runs are started by hand: see the head of that file.

import { expect, test } from "bun:test";

import { reviewer } from "../src/reviewer.ts";

const MINUTES = 60_000;

test(
  "reviewers who work at the same time never break the store",
  async () => {
    const result = await reviewer({ actions: 200, seed: 12, reviewers: 6 });
    expect(result.faults).toEqual([]);
    expect(result.head).toBeGreaterThan(50);
  },
  10 * MINUTES,
);
