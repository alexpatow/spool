// The card of an item that asks whether an identity is still one thing:
// the groups its records fall into, and the ways to answer.

import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At } from "../../act.tsx";
import type { Op, PairJudgement, ReviewCase } from "../../api.ts";
import { Code, Quiet } from "../../components/layout.tsx";
import { Ref, Source, Sources } from "../../components/refs.tsx";
import { Flag } from "../../components/forms.tsx";
import * as ShelfTable from "spool-ui/components/table";
import { facts, fieldsOf, latest, sourcesOf, valueOf } from "../../facts.ts";
import { count, recordName } from "../../format.ts";
import { CheckIcon, SplitIcon } from "../../icons.tsx";
import { stale, useRecordNames } from "../../queries.ts";
import { identityName } from "../identities.tsx";
import { shared } from "./shared.ts";
import { CardHead, labelOf, listed } from "./card.tsx";
import { Answer } from "./answers.tsx";
import { Constraints, Judgements, MOST_VALUES, Value } from "./compare.tsx";

/** A judgement as the other record of its pair reads it. */
const turned = (judgement: PairJudgement): PairJudgement => ({
  ...judgement,
  record: judgement.other,
  other: judgement.record,
});

/**
 * A `possible_split`: an identity that lost a record its source took back,
 * and whose records nothing the store knows holds together since. The
 * groups that still hold are side by side, a field to a row, with the
 * records that were taken back beside them. Under each group is the
 * decision that splits it off.
 */
export function SplitCard({
  found,
  head,
  linked,
  done,
}: {
  found: ReviewCase;
  /** Where the log ended before the case was read. */
  head: number;
  linked: boolean;
  done: (said: ReactNode) => void;
}) {
  const { item, records, candidates, judgements, constraints } = found;
  const takenBack = found.taken_back ?? [];
  const open = item.status === "open";
  const identity = candidates[0]?.identity;
  const byId = new Map(records.map((record) => [record.id, record]));
  const groups = (item.groups ?? []).map((group) => group.flatMap((id) => byId.get(id) ?? []));
  // A record that has left the identity since is not part of a decision about it.
  const still = groups.map((group) => group.filter((record) => identity && record.identity === identity.id));
  const left = still.filter((group) => group.length > 0);
  const groupOf = new Map(groups.flatMap((group, index) => group.map((record) => [record.id, index] as const)));
  const named = new Map([...records, ...takenBack].map((record) => [record.id, recordName(record)]));
  const names = useRecordNames(
    constraints.flatMap((constraint) => [constraint.a, constraint.b]),
    named,
  );
  const fields = fieldsOf([...records, ...takenBack]);
  const said = groups.map((group) => facts(latest(group)));
  const [apart, setApart] = useState(true);
  return (
    <article aria-label={`Review item ${item.id}`} {...stylex.props(shared.card)}>
      <CardHead found={found} linked={linked}>
        <p data-part="why" {...stylex.props(shared.why)}>
          They share{" "}
          {identity ? <Ref kind="identity" id={identity.id} label={identityName(identity)} /> : "an identity"}.{" "}
          {takenBack.length === 0
            ? "A record that held them together was taken back by its source."
            : takenBack.length === 1
              ? "What held them together went through a record that its source took back."
              : `What held them together went through ${count(takenBack.length, "record")} that their source took back.`}{" "}
          Each group still holds together. Nothing the store knows joins one group to another: no match found by a run,
          no must_link, and no decision by a reviewer or an agent.
        </p>
      </CardHead>
      <div {...stylex.props(shared.compare)}>
        <ShelfTable.Root aria-label={`Review item ${item.id}`}>
          <ShelfTable.Header>
            <ShelfTable.Row style={shared.row}>
              <ShelfTable.Head style={[shared.cell, shared.corner]} />
              {groups.map((group, index) => (
                <ShelfTable.Head key={index} scope="col" style={[shared.cell, shared.column]}>
                  <span data-part="record-kind" {...stylex.props(shared.columnKind)}>
                    Group {index + 1}
                    {still[index]!.length === 0 ? ", no longer in the identity" : ""}
                  </span>
                  {group.map((record) => (
                    <span key={record.id} {...stylex.props(shared.columnName)}>
                      <Ref kind="record" id={record.id} label={labelOf(record)} />
                    </span>
                  ))}
                  <span {...stylex.props(shared.columnMeta)}>
                    {count(group.length, "record")}
                    <Sources sources={sourcesOf(group)} />
                    {group.length === 1 ? <Code>{named.get(group[0]!.id)}</Code> : null}
                  </span>
                </ShelfTable.Head>
              ))}
              {takenBack.map((record) => (
                <ShelfTable.Head key={record.id} scope="col" style={[shared.cell, shared.column, shared.waits]}>
                  <span data-part="record-kind" {...stylex.props(shared.columnKind)}>
                    Taken back by its source
                  </span>
                  <span {...stylex.props(shared.columnName)}>
                    <Ref kind="record" id={record.id} label={labelOf(record)} />
                  </span>
                  <span {...stylex.props(shared.columnMeta)}>
                    <Source name={record.source} />
                    <Code>{named.get(record.id)}</Code>
                    {record.retracted != null ? (
                      <Ref kind="run" id={record.retracted} label={`run ${record.retracted}`} />
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
                {groups.map((_, index) => {
                  const held = said[index]!.find((fact) => fact.field === field)?.values ?? [];
                  return (
                    <ShelfTable.Cell key={index} style={[shared.cell, shared.value]}>
                      {held.length === 0 ? (
                        <Value value={null} />
                      ) : (
                        held
                          .slice(0, MOST_VALUES)
                          .map((value) => (
                            <Value
                              key={value.value}
                              value={value.value}
                              times={held.length > 1 ? value.records.length : undefined}
                            />
                          ))
                      )}
                      {held.length > MOST_VALUES ? (
                        <span data-part="more" {...stylex.props(shared.none)}>
                          +{held.length - MOST_VALUES} more
                        </span>
                      ) : null}
                    </ShelfTable.Cell>
                  );
                })}
                {takenBack.map((record) => (
                  <ShelfTable.Cell key={record.id} style={[shared.cell, shared.value, shared.waits]}>
                    <Value value={valueOf(record, field)} />
                  </ShelfTable.Cell>
                ))}
              </ShelfTable.Row>
            ))}
            <ShelfTable.Row data-part="found" style={shared.row}>
              <ShelfTable.Head scope="row" style={[shared.cell, shared.rowName]}>
                Judgements across groups
              </ShelfTable.Head>
              {groups.map((_, index) => (
                <ShelfTable.Cell key={index} style={[shared.cell, shared.value]}>
                  {/* Under a group: what was judged between it and another group. */}
                  <Judgements
                    judgements={judgements.flatMap((judgement) => {
                      const [mine, theirs] = [groupOf.get(judgement.record), groupOf.get(judgement.other)];
                      if (mine === undefined || theirs === undefined || mine === theirs) return [];
                      return mine === index ? [judgement] : theirs === index ? [turned(judgement)] : [];
                    })}
                    named={named}
                  />
                </ShelfTable.Cell>
              ))}
              {takenBack.map((record) => (
                <ShelfTable.Cell key={record.id} style={[shared.cell, shared.value, shared.waits]}>
                  {/* Under a record that was taken back: what it was judged against the records that remain. */}
                  <Judgements
                    judgements={judgements.filter((judgement) => judgement.other === record.id).map(turned)}
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
                {groups.map((_, index) => (
                  <ShelfTable.Cell key={index} style={[shared.cell, shared.value]}>
                    <Constraints
                      constraints={constraints.filter(({ a, b }) => groupOf.get(a) === index || groupOf.get(b) === index)}
                      names={names}
                    />
                  </ShelfTable.Cell>
                ))}
                {takenBack.map((record) => (
                  <ShelfTable.Cell key={record.id} style={[shared.cell, shared.value, shared.waits]} />
                ))}
              </ShelfTable.Row>
            ) : null}
          </ShelfTable.Body>
        </ShelfTable.Root>
      </div>
      {open && identity ? (
        <section aria-label="Answers" {...stylex.props(shared.answers)}>
          <h3 {...stylex.props(shared.partTitle)}>What each answer does</h3>
          <ol {...stylex.props(shared.answerList)}>
            {left.length > 1
              ? groups.map((_, index) => {
                  const side = still[index]!;
                  const rest = left.reduce((sum, group) => sum + group.length, 0) - side.length;
                  // Of two groups, splitting off either is the same split, so it is offered once.
                  const offered = side.length > 0 && (left.length > 2 || side !== left[0]);
                  if (!offered) return null;
                  const name = identityName(identity);
                  // The engine leaves the larger side under the identity's ID, and on a tie the side not named.
                  const moves = side.length > rest ? rest : side.length;
                  const would =
                    side.length > rest
                      ? `Group ${index + 1} keeps ${name} and its ID. The other ${count(rest, "record")} ${rest === 1 ? "moves" : "move"} to a new identity, with an ID made when the split is made.`
                      : `${side.length === 1 ? recordName(side[0]!) : `The ${count(side.length, "record")} of group ${index + 1}`} ${side.length === 1 ? "moves" : "move"} to a new identity, with an ID made when the split is made. The other ${count(rest, "record")} ${rest === 1 ? "stays" : "stay"} in ${name}, which keeps its ID.`;
                  return (
                    <Answer
                      key={index}
                      name="split"
                      title={`They are different things: split off group ${index + 1}`}
                      does={[
                        would,
                        apart
                          ? "With the two sides kept apart, as is ticked in the dialog, one cannot_link between a record of each side is recorded, so no changeset can merge them again while it stands."
                          : "With the two sides not kept apart, nothing is recorded between them. A match run never merges identities, so only a person can put them back together.",
                        `Undo it from the log and the ${count(moves, "record")} ${moves === 1 ? "moves" : "move"} back to ${name}${apart ? " and the cannot_link goes" : ""}. The new identity is not taken back: its ID stays, with no records. The question is not asked again.`,
                      ]}
                      action={
                        <Act
                          stale={stale.store}
                          label={`Split off group ${index + 1}`}
                          icon={<SplitIcon />}
                          size="sm"
                          variant="default"
                          title={`Split off group ${index + 1}`}
                          about={would}
                          why="Reason"
                          act={async (client, { reason }) => {
                            const report = await client.split(identity.id, {
                              reason,
                              base: head,
                              records: side.map((record) => record.id),
                              keep_apart: apart,
                            });
                            return (
                              <>
                                Split at <At committed={report.committed} />:{" "}
                                {count(report.moved.length, "record")} moved to{" "}
                                <Ref kind="identity" id={report.identity} />.
                              </>
                            );
                          }}
                          done={done}
                        >
                          <Flag label="Keep the two sides apart in later runs" checked={apart} onChange={setApart} />
                        </Act>
                      }
                    />
                  );
                })
              : null}
            {left.length > 1 ? (
              <Answer
                name="keep"
                title="They are one thing: keep them as one"
                does={[
                  `A must_link is recorded between ${recordName(left[0]![0]!)} and ${listed(left.slice(1).map((group) => recordName(group[0]!)))}, one record of each group. ${identityName(identity)} stays as it is, with all ${count(left.reduce((sum, group) => sum + group.length, 0), "record")} and its ID.`,
                  left.length > 2
                    ? "The must_links hold the groups together while the records they name stay: the question is not asked again when other records come and go, and it can be if a source takes back one of those."
                    : "The must_link holds the groups together while the two records it names stay: the question is not asked again when other records come and go, and it can be if a source takes back one of those two.",
                  "Undo it from the log and the must_link goes. The question is not asked again then.",
                ]}
                action={
                  <Act
                    stale={stale.store}
                    label="Keep as one"
                    icon={<CheckIcon />}
                    title="Keep as one identity"
                    about="Records that a record of each group must share an identity (must_link), so the groups hold together and the question is answered."
                    why="Reason"
                    act={async (client, { reason }) => {
                      const [first, ...others] = left;
                      const committed = await client.commit({
                        reason,
                        base: head,
                        ops: others.map(
                          (group): Op => ({ op: "constrain", kind: "must_link", a: first![0]!.id, b: group[0]!.id }),
                        ),
                      });
                      return (
                        <>
                          Kept as one identity at <At committed={committed} />.
                        </>
                      );
                    }}
                    done={done}
                  />
                }
              />
            ) : null}
            <Answer
              name="dismiss"
              title="Not sure: dismiss it"
              does={[
                `${identityName(identity)} stays as it is, and nothing is recorded about whether its records are one thing.`,
                "The question is asked again if another of its records is taken back by its source, or one comes back.",
              ]}
              action={
                <Act
                  stale={stale.store}
                  label="Dismiss"
                  variant="ghost"
                  title={`Dismiss review item ${item.id}`}
                  about="The identity stays as it is, and nothing is recorded about whether its records are one thing."
                  why="Reason"
                  act={async (client, { reason }) => {
                    await client.dismissReview(item.id, { reason });
                    return "Dismissed.";
                  }}
                  done={done}
                />
              }
            />
            <Answer
              name="leave"
              title="Leave it open"
              does={[
                "Nothing changes, and no match run looks at it: its records have an identity. It closes by itself if the record that was taken back comes back, or a new record matches more than one group.",
              ]}
            />
          </ol>
          <div {...stylex.props(shared.checked)}>
            <Quiet>
              A split is refused, changing nothing, if the identity's records changed since this screen was read.
              Keeping them as one or dismissing is not checked against what changed since.
            </Quiet>
          </div>
        </section>
      ) : null}
    </article>
  );
}
