// What a run found for the pairs of a review item, put in the order a
// reviewer reads it: what speaks against a match comes first, so that it is
// among what is written out when there is more than a card has room for.

import type { PairJudgement } from "./api.ts";

type Verdict = PairJudgement["verdict"];

const ORDER: Verdict[] = ["no_match", "unsure", "match"];

/** The judgements that say no first, then those that are unsure, and otherwise in the order they came. */
export function foremost<Judged extends Pick<PairJudgement, "verdict">>(judgements: Judged[]): Judged[] {
  return ORDER.flatMap((verdict) => judgements.filter((judgement) => judgement.verdict === verdict));
}

/** How many of the judgements gave each verdict, in words: "1 no match, 26 match". */
export function tally(judgements: Array<Pick<PairJudgement, "verdict">>): string {
  return ORDER.flatMap((verdict) => {
    const times = judgements.filter((judgement) => judgement.verdict === verdict).length;
    return times === 0 ? [] : [`${times} ${verdict.replaceAll("_", " ")}`];
  }).join(", ");
}
