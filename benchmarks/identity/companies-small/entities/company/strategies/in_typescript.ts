// The rules of this entity type, written in TypeScript.
//
// It judges the candidates the block rules found exactly as the SQL files
// under `rules/` do, and as `in_python.py` does, so a run with this strategy
// commits what a run with the built-in `rules` commits.
//
//   spool bench benchmarks/identity/companies-small --strategy in_typescript

import { strategy } from "spool-strategy";
import type { Judgement } from "spool-strategy";

const same = (left: unknown, right: unknown): boolean => left !== null && left === right;

await strategy(async (task) => {
  const judgements: Judgement[] = [];
  for (const pair of await task.candidates()) {
    // rules/no_match: two registry identifiers that differ are two legal
    // entities, however alike the names and domains are.
    const different: string[] = [];
    if (
      pair.left_registry_id !== null &&
      pair.right_registry_id !== null &&
      pair.left_registry_id !== pair.right_registry_id
    ) {
      different.push("different_registry_id");
    }

    // rules/match
    const agree: string[] = [];
    const country = same(pair.left_country, pair.right_country);
    if (same(pair.left_domain, pair.right_domain)) agree.push("same_domain");
    if (same(pair.left_name_key, pair.right_name_key) && country) agree.push("same_name_and_country");
    if (same(pair.left_registry_id, pair.right_registry_id)) agree.push("same_registry_id");
    if (
      (same(pair.left_name_key, pair.right_trading_key) || same(pair.left_trading_key, pair.right_name_key)) &&
      country
    ) {
      agree.push("trading_name_and_country");
    }

    if (different.length > 0 || agree.length > 0) {
      judgements.push({
        left_id: pair.left_id,
        right_id: pair.right_id,
        verdict: different.length > 0 ? "no_match" : "match",
        detail: { no_match: different, match: agree },
      });
    }
  }
  return judgements;
});
