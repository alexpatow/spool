// A small run of the replay: a public benchmark resolved in one process and
// again through a server. The big ones are started by hand: see the head of
// `src/replay.ts`.

import { expect, test } from "bun:test";

import { join } from "node:path";

import { replay } from "../src/replay.ts";
import { repository } from "../src/server.ts";

const MINUTES = 60_000;

test(
  "public records resolved in one process and over HTTP end as the same identities",
  async () => {
    // 19,375 MusicBrainz records in five sources, downloaded the first time.
    const result = await replay(join(repository, "benchmarks/resolution/music/20k.toml"), 1, () => {});
    expect(result.differences).toEqual([]);
    expect(result.faults).toEqual([]);
    expect(result.verified).toStartWith("verified ");
    expect(result.records).toBe(19_375);
    // The rules are the same each time, so the result is: no two songs are
    // made one, and most of what is one song is found.
    expect(result.identities).toBe(11_073);
    expect(result.quality).toMatchObject({ true_entities: 10_000, false_merges: 0, false_splits: 989, unassigned: 0 });
  },
  10 * MINUTES,
);
