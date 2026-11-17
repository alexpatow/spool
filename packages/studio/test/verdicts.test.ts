// What a run found for many pairs, put in the order a reviewer reads it.

import { describe, expect, test } from "bun:test";

import { foremost, tally } from "../src/verdicts.ts";

const judged = (verdict: "match" | "no_match" | "unsure", other: string) => ({ verdict, other });

describe("what a run found", () => {
  test("is read with what speaks against a match first, and otherwise in the order it came", () => {
    const found = [
      judged("match", "a"),
      judged("unsure", "b"),
      judged("match", "c"),
      judged("no_match", "d"),
      judged("unsure", "e"),
      judged("no_match", "f"),
    ];
    expect(foremost(found).map(({ other }) => other)).toEqual(["d", "f", "b", "e", "a", "c"]);
    // The list it was given is left as it was.
    expect(found.map(({ other }) => other)).toEqual(["a", "b", "c", "d", "e", "f"]);
  });

  test("is counted by verdict, in that order, leaving out a verdict no pair got", () => {
    expect(tally([judged("match", "a"), judged("no_match", "b"), judged("match", "c"), judged("unsure", "d")])).toBe(
      "1 no match, 1 unsure, 2 match",
    );
    expect(tally([judged("match", "a"), judged("match", "b")])).toBe("2 match");
    expect(tally([])).toBe("");
  });
});
