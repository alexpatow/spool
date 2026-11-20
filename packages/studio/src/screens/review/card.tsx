// What every card of a review item has: what it asks, in the words of
// the records, and what came of it.

import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import type { Candidate, IdentitySummary, RecordEntry, ReviewCase, ReviewEntry } from "../../api.ts";
import { Quiet, Reason } from "../../components/layout.tsx";
import { Ref, When, Word } from "../../components/refs.tsx";
import { count, recordName, tail } from "../../format.ts";
import { colors, spacing, typography } from "spool-ui/tokens.stylex";
import { identityName } from "../identities.tsx";
import { Who } from "../../who.tsx";

/** What to call a record where a person reads it: what its payload calls it, or its source and key. */
export function labelOf(record: RecordEntry): string {
  return record.label ?? recordName(record);
}

/**
 * What to call each of the identities a case names, where they are named
 * together: two that are called the same are told apart by the end of
 * their IDs.
 */
export function namer(candidates: Candidate[]): (identity: IdentitySummary) => string {
  const names = candidates.map((candidate) => identityName(candidate.identity));
  return (identity) => {
    const name = identityName(identity);
    return names.filter((other) => other === name).length > 1 ? `${name} (…${tail(identity.id)})` : name;
  };
}

/** Names in a sentence: `a`, `a and b`, `a, b and c`. */
export function listed(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/** The records an item is about, as a sentence names them: the one by its label, or how many. */
export function theRecords(records: RecordEntry[]): string {
  return records.length === 1 ? labelOf(records[0]!) : count(records.length, "record");
}

/** What the item asks, in a line. */
function asked({ item, records, candidates }: ReviewCase): string {
  const call = namer(candidates);
  const names = candidates.map((candidate) => call(candidate.identity));
  if (item.kind === "possible_split") {
    return `Is ${names[0] ?? "this identity"} still one thing?`;
  }
  const what = theRecords(records);
  const one = records.length === 1;
  if (item.kind === "possible_merge") {
    return candidates.length === 2
      ? `${what} ${one ? "matches" : "match"} both ${names[0]} and ${names[1]}. Are they the same thing?`
      : `${what} ${one ? "matches" : "match"} ${count(candidates.length, "identity")}. Are they one thing?`;
  }
  return candidates.length === 1
    ? `Does ${what} belong in ${names[0]}?`
    : `${one ? "Does" : "Do"} ${what} belong together?`;
}

/** What every card starts with: which item it is, what it asks, and what became of it. */
export function CardHead({ found, linked, children }: { found: ReviewCase; linked: boolean; children?: ReactNode }) {
  const { item } = found;
  const open = item.status === "open";
  return (
    <header {...stylex.props(styles.cardHead)}>
      <div {...stylex.props(styles.meta)}>
        {linked ? <Ref kind="review" id={item.id} label={`Item ${item.id}`} /> : <span>Item {item.id}</span>}
        <Word value={item.kind} />
        {open ? null : <Word value={item.status} />}
        <span>{item.entity_type}</span>
        <span>
          <Ref kind="run" id={item.run} label={`run ${item.run}`} />
        </span>
      </div>
      <h2 {...stylex.props(styles.question)}>{asked(found)}</h2>
      {children}
      {open ? null : <Outcome item={item} />}
    </header>
  );
}

/** What became of an item that is no longer open. */
function Outcome({ item }: { item: ReviewEntry }) {
  if (item.status === "resolved") {
    return (
      <Quiet>
        Resolved
        {item.resolved_seq ? (
          <>
            {" "}
            at <Ref kind="log" id={item.resolved_seq} label={`log position ${item.resolved_seq}`} />
          </>
        ) : null}
        .
      </Quiet>
    );
  }
  if (item.status === "dismissed") {
    return (
      <Quiet>
        Dismissed
        {item.dismissed ? (
          <>
            {" "}
            by <Who actor={item.dismissed.by} /> at <When at={item.dismissed.at} />
            {item.dismissed.reason ? (
              <>
                {" "}
                <Reason>{item.dismissed.reason}</Reason>
              </>
            ) : null}
          </>
        ) : null}
        .
      </Quiet>
    );
  }
  return (
    <Quiet>
      Superseded
      {item.superseded_by ? (
        <>
          {" "}
          by <Ref kind="run" id={item.superseded_by} label={`run ${item.superseded_by}`} />
        </>
      ) : null}
      .
    </Quiet>
  );
}

const styles = stylex.create({
  cardHead: {
    gap: spacing["1.5"],
    padding: spacing["4"],
    display: "flex",
    flexDirection: "column",
  },
  meta: {
    columnGap: spacing["2.5"],
    rowGap: spacing["1"],
    alignItems: "center",
    color: colors.mutedForeground,
    display: "flex",
    flexWrap: "wrap",
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
  },
  question: {
    margin: 0,
    fontSize: typography.fontSizeBase,
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.01em",
    lineHeight: typography.lineHeightBase,
  },
});
