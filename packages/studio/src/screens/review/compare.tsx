// The record that waits and the identities it matched, field by field,
// with what the run judged and what a reviewer decided before.

import * as stylex from "@stylexjs/stylex";
import { useState } from "react";

import type { CaseConstraint, PairJudgement, ReviewCase } from "../../api.ts";
import { Code } from "../../components/layout.tsx";
import { Onward, Ref, Source, Sources, Word } from "../../components/refs.tsx";
import { Button } from "spool-ui/components/button";
import * as ShelfTable from "spool-ui/components/table";
import { facts, fieldsOf, latest, same, valueOf } from "../../facts.ts";
import { count, recordName } from "../../format.ts";
import { CheckIcon } from "../../icons.tsx";
import { useRecordNames } from "../../queries.ts";
import { radius, spacing } from "spool-ui/tokens.stylex";
import { tones } from "../../styles/tones.stylex";
import { foremost, tally } from "../../verdicts.ts";
import { identityName } from "../identities.tsx";
import { shared } from "./shared.ts";
import { labelOf } from "./card.tsx";

/** The most values of one field that a card writes out for an identity. The identity's own screen has them all. */
export const MOST_VALUES = 4;

/**
 * The records an item names beside the identities they matched, a field to
 * a row, so that what agrees and what does not is read across. Under each
 * column is the decision that column stands for.
 */
export function Compare({ found }: { found: ReviewCase }) {
  const { records, candidates, judgements, constraints } = found;
  const fields = fieldsOf([...records, ...candidates.flatMap((candidate) => candidate.members)]);
  // What a candidate is compared with: the first record the item names.
  const first = records[0];
  const named = new Map(
    [...records, ...candidates.flatMap((candidate) => candidate.members)].map((record) => [
      record.id,
      recordName(record),
    ]),
  );
  const said = candidates.map((candidate) => facts(latest(candidate.members)));
  // A constraint may name a member that is not among those shown, or a record of neither.
  const names = useRecordNames(
    constraints.flatMap((constraint) => [constraint.a, constraint.b]),
    named,
  );
  const matched = new Set(candidates.map((candidate) => candidate.identity.id));
  return (
    <div {...stylex.props(shared.compare)}>
      <ShelfTable.Root aria-label={`Review item ${found.item.id}`}>
        <ShelfTable.Header>
          <ShelfTable.Row style={shared.row}>
            <ShelfTable.Head style={[shared.cell, shared.corner]} />
            {records.map((record) => (
              <ShelfTable.Head key={record.id} scope="col" style={[shared.cell, shared.column, shared.waits]}>
                <span data-part="record-kind" {...stylex.props(shared.columnKind)}>
                  {record.identity ? "Record" : record.retracted != null ? "Taken back by its source" : "Unassigned record"}
                </span>
                <span {...stylex.props(shared.columnName)}>
                  <Ref kind="record" id={record.id} label={labelOf(record)} />
                </span>
                <span {...stylex.props(shared.columnMeta)}>
                  <Source name={record.source} />
                  <Code>{named.get(record.id)}</Code>
                </span>
              </ShelfTable.Head>
            ))}
            {candidates.map((candidate) => (
              <ShelfTable.Head key={candidate.identity.id} scope="col" style={[shared.cell, shared.column]}>
                <span {...stylex.props(shared.columnKind)}>Matched identity</span>
                <span {...stylex.props(shared.columnName)}>
                  <Ref kind="identity" id={candidate.identity.id} label={identityName(candidate.identity)} />
                </span>
                <span {...stylex.props(shared.columnMeta)}>
                  {count(candidate.identity.members, "record")}
                  <Sources sources={candidate.identity.sources} />
                  {/* The case holds some of the records of a large identity: what is compared below is theirs. */}
                  {candidate.members.length < candidate.identity.members ? (
                    <span data-part="sample">{candidate.members.length} shown</span>
                  ) : null}
                </span>
              </ShelfTable.Head>
            ))}
          </ShelfTable.Row>
        </ShelfTable.Header>
        <ShelfTable.Body>
          {fields.map((field) => (
            <ShelfTable.Row key={field} data-field={field} style={shared.row}>
              <ShelfTable.Head scope="row" style={[shared.cell, shared.fieldName]}>
                {field}
              </ShelfTable.Head>
              {records.map((record) => (
                <ShelfTable.Cell key={record.id} style={[shared.cell, shared.value, shared.waits]}>
                  <Value value={valueOf(record, field)} />
                </ShelfTable.Cell>
              ))}
              {candidates.map((candidate, index) => {
                const held = said[index]!.find((fact) => fact.field === field)?.values ?? [];
                const baseline = first ? valueOf(first, field) : null;
                // A value the record that waits holds too comes first, so that it is among those written out.
                const agreeing = held.filter((value) => baseline !== null && same(value.value, baseline));
                const ordered = [...agreeing, ...held.filter((value) => !agreeing.includes(value))];
                return (
                  <ShelfTable.Cell key={candidate.identity.id} style={[shared.cell, shared.value]}>
                    {held.length === 0 ? (
                      <Value value={null} />
                    ) : (
                      ordered
                        .slice(0, MOST_VALUES)
                        .map((value) => (
                          <Value
                            key={value.value}
                            value={value.value}
                            agrees={baseline === null ? undefined : agreeing.includes(value)}
                            times={held.length > 1 ? value.records.length : undefined}
                          />
                        ))
                    )}
                    {ordered.length > MOST_VALUES ? (
                      <span data-part="more" {...stylex.props(shared.none)}>
                        +{ordered.length - MOST_VALUES} more
                      </span>
                    ) : null}
                  </ShelfTable.Cell>
                );
              })}
            </ShelfTable.Row>
          ))}
          <ShelfTable.Row data-part="found" style={shared.row}>
            <ShelfTable.Head scope="row" style={[shared.cell, shared.rowName]}>
              Judgements
            </ShelfTable.Head>
            {records.map((record) => (
              <ShelfTable.Cell key={record.id} style={[shared.cell, shared.value, shared.waits]}>
                <Judgements
                  judgements={judgements.filter(
                    (judgement) =>
                      judgement.record === record.id && records.some((other) => other.id === judgement.other),
                  )}
                  named={named}
                />
              </ShelfTable.Cell>
            ))}
            {candidates.map((candidate) => (
              <ShelfTable.Cell key={candidate.identity.id} style={[shared.cell, shared.value]}>
                {/* Against any of its records: the case does not hold them all, so the judgement says whose it is. */}
                <Judgements
                  judgements={judgements.filter(
                    (judgement) =>
                      judgement.identity === candidate.identity.id &&
                      !records.some((other) => other.id === judgement.other),
                  )}
                  named={named}
                />
              </ShelfTable.Cell>
            ))}
          </ShelfTable.Row>
          {constraints.length > 0 ? (
            <ShelfTable.Row data-part="decided" style={shared.row}>
              <ShelfTable.Head scope="row" style={[shared.cell, shared.rowName]}>
                Constraints
              </ShelfTable.Head>
              {records.map((record) => (
                <ShelfTable.Cell key={record.id} style={[shared.cell, shared.value, shared.waits]}>
                  {/* Under a record: what was decided about it and a record of none of the identities. */}
                  <Constraints
                    constraints={constraints.filter(
                      ({ a, b, identities }) =>
                        (a === record.id || b === record.id) &&
                        !identities.some((identity) => identity !== null && matched.has(identity)),
                    )}
                    names={names}
                  />
                </ShelfTable.Cell>
              ))}
              {candidates.map((candidate) => (
                <ShelfTable.Cell key={candidate.identity.id} style={[shared.cell, shared.value]}>
                  <Constraints
                    constraints={constraints.filter(({ identities }) => identities.includes(candidate.identity.id))}
                    names={names}
                  />
                </ShelfTable.Cell>
              ))}
            </ShelfTable.Row>
          ) : null}
        </ShelfTable.Body>
      </ShelfTable.Root>
    </div>
  );
}

/** One value of a field. Beside a candidate's, whether the record that waits holds the same. */
export function Value({ value, agrees, times }: { value: string | null; agrees?: boolean; times?: number }) {
  if (value === null) return <span {...stylex.props(shared.none)}>empty</span>;
  return (
    <span data-agrees={agrees} {...stylex.props(styles.held)}>
      {agrees === undefined ? null : agrees ? (
        <CheckIcon aria-label="Same" aria-hidden={false} {...stylex.props(styles.agrees)} />
      ) : (
        <span role="img" aria-label="Different" {...stylex.props(styles.differs)} />
      )}
      <span>
        {value}
        {times !== undefined ? <span {...stylex.props(shared.times)}> ×{times}</span> : null}
      </span>
    </span>
  );
}

/** What the constraints in force say of some pairs, each with the changeset that made it. */
export function Constraints({ constraints, names }: { constraints: CaseConstraint[]; names: Map<string, string> }) {
  if (constraints.length === 0) return <span {...stylex.props(shared.none)}>none</span>;
  return (
    <span {...stylex.props(styles.judgements)}>
      {constraints.map((constraint) => (
        <span
          key={`${constraint.a}-${constraint.b}`}
          data-part="constraint"
          data-kind={constraint.kind}
          {...stylex.props(styles.judgement)}
        >
          <Word value={constraint.kind} />
          <span>
            <Ref kind="record" id={constraint.a} label={names.get(constraint.a)} /> and{" "}
            <Ref kind="record" id={constraint.b} label={names.get(constraint.b)} />, since{" "}
            <Ref kind="log" id={constraint.from} label={`log position ${constraint.from}`} />
          </span>
          <Onward to="/explain" search={{ a: constraint.a, b: constraint.b }}>
            explain
          </Onward>
        </span>
      ))}
    </span>
  );
}

/** The rules a strategy named with a verdict, when what it kept says so. */
export function rulesOf(detail: unknown, verdict: string): string[] {
  if (typeof detail !== "object" || detail === null) return [];
  const listed = (detail as Record<string, unknown>)[verdict === "no_match" ? "no_match" : "match"];
  return Array.isArray(listed) ? listed.filter((rule) => typeof rule === "string") : [];
}

/** What the latest run made of some pairs: its verdict on each, and the rule or the score behind it. */
/** The most judgements a cell writes out before it is asked for the rest. */
const MOST_JUDGEMENTS = 6;

/**
 * What the run found for some pairs. Those that speak against a match come
 * first. When there are many, the first are written out and the rest are
 * counted by verdict, with a button that writes them out: none is left out
 * unsaid.
 */
export function Judgements({ judgements, named }: { judgements: PairJudgement[]; named: Map<string, string> }) {
  const [all, setAll] = useState(false);
  const ordered = foremost(judgements);
  const shown = all ? ordered : ordered.slice(0, MOST_JUDGEMENTS);
  const more = ordered.slice(shown.length);
  // One that is against a record the case does not hold is named by looking the record up.
  const names = useRecordNames(
    shown.map((judgement) => judgement.other),
    named,
  );
  if (judgements.length === 0) return <span {...stylex.props(shared.none)}>none</span>;
  return (
    <span {...stylex.props(styles.judgements)}>
      {shown.map((judgement) => {
        const by = rulesOf(judgement.detail, judgement.verdict);
        return (
          <span
            key={`${judgement.record}-${judgement.other}`}
            data-part="judgement"
            {...stylex.props(styles.judgement)}
          >
            <Word value={judgement.verdict} />
            <span>
              with <Ref kind="record" id={judgement.other} label={names.get(judgement.other)} />
              {by.length > 0 ? ` by ${by.join(", ")}` : ""}
              {judgement.score !== null && judgement.score !== undefined ? `, score ${judgement.score}` : ""}
            </span>
            <Onward to="/explain" search={{ a: judgement.record, b: judgement.other }}>
              explain
            </Onward>
          </span>
        );
      })}
      {more.length > 0 ? (
        <span {...stylex.props(styles.judgement)}>
          <span data-part="more" {...stylex.props(shared.none)}>
            +{more.length} more: {tally(more)}
          </span>
          <Button variant="ghost" size="sm" onClick={() => setAll(true)}>
            Show all {ordered.length}
          </Button>
        </span>
      ) : null}
    </span>
  );
}

const styles = stylex.create({
  held: {
    gap: spacing["2"],
    alignItems: "baseline",
    display: "flex",
  },
  agrees: {
    alignSelf: "center",
    color: tones.goodMark,
    flexShrink: 0,
  },
  differs: {
    borderColor: tones.waitsMark,
    borderRadius: radius.full,
    borderStyle: "solid",
    borderWidth: 1.5,
    alignSelf: "center",
    boxSizing: "border-box",
    flexShrink: 0,
    marginInline: "0.1875rem",
    height: "0.5rem",
    width: "0.5rem",
  },
  judgements: {
    gap: spacing["1.5"],
    display: "flex",
    flexDirection: "column",
  },
  judgement: {
    columnGap: spacing["2"],
    rowGap: spacing["1"],
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
  },
});
