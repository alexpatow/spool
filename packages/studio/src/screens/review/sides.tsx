// The identities a record matched, side by side: what each is called,
// some of its records, and what tells them apart.

import * as stylex from "@stylexjs/stylex";

import type { Candidate, ReviewCase } from "../../api.ts";
import { Code } from "../../components/layout.tsx";
import { Ref, Sources } from "../../components/refs.tsx";
import { facts, latest, same } from "../../facts.ts";
import { count, recordName } from "../../format.ts";
import { colors, spacing, typography } from "spool-ui/tokens.stylex";
import { shared } from "./shared.ts";
import { labelOf, listed, namer } from "./card.tsx";
import { rulesOf } from "./compare.tsx";

/** The most members of an identity a card names as a sample. */
const SAMPLE = 5;

/** The most fields a side names where the identities differ. */
const MOST_DIFFERENCES = 3;

/**
 * The fields in which what most of the shown records of each identity say
 * is not the same for all of them, with what each says. A record's own key
 * is left out: it tells every record from every other.
 */
function differences(candidates: Candidate[]): Array<{ field: string; values: Array<string | null> }> {
  const said = candidates.map((candidate) =>
    facts(latest(candidate.members)).filter((fact) =>
      fact.values.every(({ value, records }) => records.every((record) => record.key !== value)),
    ),
  );
  const fields = [...new Set(said.flatMap((facts) => facts.map((fact) => fact.field)))];
  return fields
    .map((field) => ({
      field,
      values: said.map((facts) => facts.find((fact) => fact.field === field)?.values[0]?.value ?? null),
    }))
    .filter(({ values }) => values.some((value) => !same(value, values[0] ?? null)))
    .slice(0, MOST_DIFFERENCES);
}

/** The identities the records matched, side by side: what each is, a few of its records, and what links them. */
export function SideBySide({ found }: { found: ReviewCase }) {
  const { records, candidates, judgements } = found;
  const differ = differences(candidates);
  const held = new Set(records.map((record) => record.id));
  const call = namer(candidates);
  return (
    <section aria-label="The identities" data-part="identities" {...stylex.props(styles.sides)}>
      {candidates.map((candidate) => {
        const { identity, members } = candidate;
        const mine = judgements.filter(
          (judgement) => judgement.identity === identity.id && held.has(judgement.record),
        );
        const matches = mine.filter((judgement) => judgement.verdict === "match");
        const against = mine.filter((judgement) => judgement.verdict === "no_match");
        const rulesHeld = [...new Set(matches.flatMap((judgement) => rulesOf(judgement.detail, "match")))];
        const others = new Set(matches.map((judgement) => judgement.other));
        return (
          <div key={identity.id} data-part="side" {...stylex.props(styles.side)}>
            <span {...stylex.props(shared.columnKind)}>Matched identity</span>
            <span {...stylex.props(shared.columnName)}>
              <Ref kind="identity" id={identity.id} label={call(identity)} />
            </span>
            <span data-part="size" {...stylex.props(shared.columnMeta)}>
              {count(identity.members, "record")}
              <Sources sources={identity.sources} />
            </span>
            <ul data-part="members" aria-label={`Some records of ${call(identity)}`} {...stylex.props(styles.sample)}>
              {members.slice(0, SAMPLE).map((record) => (
                <li key={record.id}>
                  <Ref kind="record" id={record.id} label={labelOf(record)} />{" "}
                  <span {...stylex.props(shared.times)}>{recordName(record)}</span>
                </li>
              ))}
              {identity.members > SAMPLE ? (
                <li {...stylex.props(shared.none)}>and {count(identity.members - Math.min(SAMPLE, members.length), "more record")}</li>
              ) : null}
            </ul>
            {differ.length > 0 ? (
              <p data-part="differs" {...stylex.props(styles.links)}>
                Unlike the other{candidates.length > 2 ? "s" : ""}:{" "}
                {differ.map(({ field, values }, at) => (
                  <span key={field}>
                    {at > 0 ? ", " : ""}
                    <Code>{field}</Code> {values[candidates.indexOf(candidate)] ?? "empty"}
                  </span>
                ))}
              </p>
            ) : null}
            <p data-part="links" {...stylex.props(styles.links)}>
              {matches.length === 0
                ? "No judgement of this case links the records to it."
                : `Linked by ${matches.length === 1 ? "1 match" : `${matches.length} matches`} with ${others.size} of its records${rulesHeld.length > 0 ? `, by ${listed(rulesHeld)}` : ""}.`}
              {against.length > 0 ? ` ${against.length} no_match against it.` : ""}
            </p>
          </div>
        );
      })}
    </section>
  );
}

const styles = stylex.create({
  sides: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(16rem, 1fr))",
  },
  side: {
    borderInlineStartColor: colors.border,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: { default: 1, ":first-child": 0 },
    gap: spacing["1"],
    padding: spacing["4"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  sample: {
    margin: 0,
    gap: spacing["1"],
    marginTop: spacing["2"],
    padding: 0,
    display: "flex",
    flexDirection: "column",
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    listStyleType: "none",
    overflowWrap: "anywhere",
  },
  links: {
    margin: 0,
    color: colors.mutedForeground,
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    marginTop: spacing["2"],
  },
});
