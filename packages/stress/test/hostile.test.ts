// A small run of `src/hostile.ts`, so that the stress test itself keeps
// working and a fault it once found does not come back. The big runs are
// started by hand: see the head of that file.

import { expect, test } from "bun:test";

import { hostile } from "../src/hostile.ts";

const MINUTES = 60_000;

test(
  "requests nobody should send are refused, and the server goes on working",
  async () => {
    const result = await hostile();
    expect(result.faults).toEqual([]);
    expect(result.cases).toBeGreaterThan(150);
  },
  5 * MINUTES,
);
