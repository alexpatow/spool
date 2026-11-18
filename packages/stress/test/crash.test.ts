// Small runs of `src/crash.ts`, so that the stress test itself keeps working
// and a fault it once found does not come back. The big runs are started by
// hand: see the head of that file.

import { expect, test } from "bun:test";

import { rmSync } from "node:fs";
import { join } from "node:path";

import { crash, steps } from "../src/crash.ts";
import { generate } from "../src/generate.ts";
import { scratch, serve } from "../src/server.ts";
import { writeProject } from "../src/write.ts";

const MINUTES = 60_000;

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
