import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done, Mistake } from "../act.tsx";
import type {
  Candidate,
  CaseConstraint,
  ObservationRecord,
  Op,
  PairJudgement,
  ReviewCase,
  ReviewRecord,
  ReviewStatus,
} from "../api.ts";
import {
  Choose,
  Code,
  Fields,
  Filters,
  Labelled,
  Load,
  Onward,
  Page,
  Quiet,
  Reason,
  Ref,
  Select,
  Source,
  Sources,
  When,
  Word,
} from "../components.tsx";
import { Button } from "../components/ui/button";
import * as ShelfTable from "../components/ui/table";
import { facts, fieldsOf, latest, same, valueOf } from "../facts.ts";
import { count, observationName, short } from "../format.ts";
import { useLoad, useNames, useStudio } from "../hooks.ts";
import { CheckIcon, InboxIcon, MergeIcon, PlusIcon } from "../icons.tsx";
import { href } from "../router.ts";
import type { Route } from "../router.ts";
import { colors, radius, spacing, typography } from "../styles/shelf/tokens.stylex";
import { tones } from "../styles/tones.stylex";
import { foremost, tally } from "../verdicts.ts";
import { identityName } from "./identities.tsx";

const STATUSES: ReviewStatus[] = ["open", "resolved", "dismissed", "superseded"];

/** How many items are shown with all a reviewer decides them from, before the reader asks for more. */
const AT_ONCE = 10;

export function ReviewItems({ route, types }: { route: Route; types: string[] }) {
  const { type } = route.query;
  const status = route.query.status ?? "open";
  const { info } = useStudio();
  const [shown, setShown] = useState(AT_ONCE);
  // An item that was decided leaves the list of those that are open, so
  // what came of it is said above the list.
  const [done, setDone] = useState<ReactNode>(null);
  const items = useLoad((client) =>
    client.reviewItems({ status: status === "all" ? undefined : (status as ReviewStatus), entity_type: type }),
  );
  const waiting = info.store.entity_types
    .filter((entity) => type === undefined || entity.name === type)
    .reduce((sum, entity) => sum + entity.open_review_items, 0);
  return (
    <Page title="Review">
      <div {...stylex.props(styles.narrow)}>
        <Choose
          label="Status"
          choices={[...STATUSES, "all"].map((value) => ({
            label: value === "open" ? "Open" : value[0]!.toUpperCase() + value.slice(1),
            address: href(["review"], { ...route.query, status: value === "open" ? undefined : value }),
            current: status === value,
            count: value === "open" ? waiting : undefined,
          }))}
        />
        {types.length > 1 ? (
          <Filters
            path={["review"]}
            query={route.query}
            selects={[{ name: "type", label: "Entity type", choices: types.map((value) => ({ value })) }]}
          />
        ) : null}
      </div>
      <Done>{done}</Done>
      <Load state={items}>
        {(items) =>
          items.length === 0 ? (
            <div data-part="empty" {...stylex.props(styles.nothing)}>
              <InboxIcon {...stylex.props(styles.nothingIcon)} />
              <strong>{status === "open" ? "No open review items." : "No review items."}</strong>
            </div>
          ) : (
            <>
              {items.slice(0, shown).map((item) => (
                <Case key={item.id} id={item.id} linked done={setDone} />
              ))}
              {items.length > shown ? (
                <div {...stylex.props(styles.more)}>
                  <Button variant="outline" size="sm" onClick={() => setShown(shown + AT_ONCE)}>
                    Show {Math.min(AT_ONCE, items.length - shown)} more of {items.length}
                  </Button>
                </div>
              ) : null}
            </>
          )
        }
      </Load>
    </Page>
  );
}

export function ReviewItem({ id }: { id: string }) {
  const [done, setDone] = useState<ReactNode>(null);
  return (
    <Page
      title={`Review item ${id}`}
      kind={
        <>
          <InboxIcon /> Review
        </>
      }
    >
      <Done>{done}</Done>
      <Case id={Number(id)} detail done={setDone} />
    </Page>
  );
}

/** One review item, read with what a reviewer decides it from. */
function Case({
  id,
  linked = false,
  detail = false,
  done,
}: {
  id: number;
  linked?: boolean;
  detail?: boolean;
  done: (said: ReactNode) => void;
}) {
  // Where the log ended is read before the case, so that a merge decided from the card is
  // written against what the card showed, and the engine refuses it if that changed since.
  const loaded = useLoad(async (client) => {
    const { store } = await client.info();
    return { head: store.head, found: await client.reviewCase(id) };
  });
  return (
    <Load state={loaded}>
      {({ head, found }) => (
        <>
          {detail ? <Details item={found.item} /> : null}
          <CaseCard found={found} head={head} linked={linked} done={done} />
        </>
      )}
    </Load>
  );
}

function Details({ item }: { item: ReviewRecord }) {
  return (
    <Fields
      rows={[
        ["Kind", <Word value={item.kind} />],
        ["Status", <Word value={item.status} />],
        ["Entity type", item.entity_type],
        ["Run", <Ref kind="run" id={item.run} label={`reconcile run ${item.run}`} />],
        [
          "Resolved at",
          item.resolved_seq ? (
            <Ref kind="log" id={item.resolved_seq} label={`log position ${item.resolved_seq}`} />
          ) : null,
        ],
        [
          "Superseded by",
          item.superseded_by ? (
            <Ref kind="run" id={item.superseded_by} label={`reconcile run ${item.superseded_by}`} />
          ) : null,
        ],
        [
          "Dismissed by",
          item.dismissed ? (
            <>
              <Code>{item.dismissed.by}</Code> at <When at={item.dismissed.at} />
              {item.dismissed.reason ? (
                <>
                  {" "}
                  <Reason>{item.dismissed.reason}</Reason>
                </>
              ) : null}
            </>
          ) : null,
        ],
      ]}
    />
  );
}

/** What to call a record where a person reads it: what its payload calls it, or its source and key. */
function recordName(record: ObservationRecord): string {
  return record.label ?? observationName(record);
}

/**
 * What the constraints in force rule out for a case. The engine refuses a
 * changeset that would break one, so the screen does not offer it.
 */
interface Ruled {
  /** Whether a record may not be placed in an identity: a member of it must not share an identity with it. */
  barred: (observation: string, identity: string) => boolean;
  /** Whether the identities cannot be merged with the records that wait placed in the one that stands. */
  apart: boolean;
}

function ruled({ candidates, constraints }: ReviewCase, waiting: ObservationRecord[]): Ruled {
  const matched = new Set(candidates.map((candidate) => candidate.identity.id));
  const waits = new Set(waiting.map((observation) => observation.id));
  const kept = constraints.filter((constraint) => constraint.kind === "cannot_link");
  // After the merge, a record is in the one identity if it waits or belongs to one that was matched.
  const joined = (observation: string, identity: string | null) =>
    waits.has(observation) || (identity !== null && matched.has(identity));
  return {
    barred: (observation, identity) =>
      kept.some(
        ({ a, b, identities }) =>
          (a === observation && identities[1] === identity) || (b === observation && identities[0] === identity),
      ),
    apart: kept.some(({ a, b, identities }) => joined(a, identities[0]) && joined(b, identities[1])),
  };
}

/** What the item is about, in a line. */
function asked({ item, observations, candidates }: ReviewCase): string {
  const what = observations.length === 1 ? recordName(observations[0]!) : count(observations.length, "record");
  return item.kind === "possible_merge"
    ? `${what} matched ${count(candidates.length, "identity")}`
    : `Conflicting evidence about ${what}`;
}

function CaseCard({
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
  const { item, observations, candidates } = found;
  const open = item.status === "open";
  const waiting = observations.filter((observation) => !observation.identity);
  const rules = ruled(found, waiting);
  return (
    <article aria-label={`Review item ${item.id}`} {...stylex.props(styles.card)}>
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
        {open ? null : <Outcome item={item} />}
      </header>
      <Compare found={found} open={open} waiting={waiting} rules={rules} done={done} />
      {open ? (
        <footer {...stylex.props(styles.cardFoot)}>
          {waiting.length > 0 && candidates.length > 1 ? (
            rules.apart ? (
              <span data-part="apart" {...stylex.props(styles.apart)}>
                Cannot be merged: a cannot_link constraint is between them.
              </span>
            ) : (
              <MergeAndPlace found={found} head={head} waiting={waiting} done={done} />
            )
          ) : null}
          <Act
            label="Dismiss"
            variant="ghost"
            title={`Dismiss review item ${item.id}`}
            about="Its records stay unassigned, and later runs skip them."
            who
            why="Reason"
            act={async (client, { actor, reason }) => {
              await client.dismissReview(item.id, { actor, reason });
              return "Dismissed.";
            }}
            done={done}
          />
        </footer>
      ) : null}
    </article>
  );
}

/** What became of an item that is no longer open. */
function Outcome({ item }: { item: ReviewRecord }) {
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
            by <Code>{item.dismissed.by}</Code> at <When at={item.dismissed.at} />
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

/** In a new identity, as a choice of where an observation goes. */
const NEW = "new";

/** The most values of one field that a card writes out for an identity. The identity's own screen has them all. */
const MOST_VALUES = 4;

/**
 * The records an item names beside the identities they matched, a field to
 * a row, so that what agrees and what does not is read across. Under each
 * column is the decision that column stands for.
 */
function Compare({
  found,
  open,
  waiting,
  rules,
  done,
}: {
  found: ReviewCase;
  open: boolean;
  waiting: ObservationRecord[];
  rules: Ruled;
  done: (said: ReactNode) => void;
}) {
  const { observations, candidates, judgements, constraints } = found;
  const fields = fieldsOf([...observations, ...candidates.flatMap((candidate) => candidate.members)]);
  // What a candidate is compared with: the first record the item names.
  const first = observations[0];
  const named = new Map(
    [...observations, ...candidates.flatMap((candidate) => candidate.members)].map((record) => [
      record.id,
      observationName(record),
    ]),
  );
  const said = candidates.map((candidate) => facts(latest(candidate.members)));
  // A constraint may name a member that is not among those shown, or a record of neither.
  const names = useNames(
    constraints.flatMap((constraint) => [constraint.a, constraint.b]),
    named,
  );
  const matched = new Set(candidates.map((candidate) => candidate.identity.id));
  return (
    <div {...stylex.props(styles.compare)}>
      <ShelfTable.Root aria-label={`Review item ${found.item.id}`}>
        <ShelfTable.Header>
          <ShelfTable.Row style={styles.row}>
            <ShelfTable.Head style={[styles.cell, styles.corner]} />
            {observations.map((observation) => (
              <ShelfTable.Head key={observation.id} scope="col" style={[styles.cell, styles.column, styles.waits]}>
                <span {...stylex.props(styles.columnKind)}>
                  {observation.identity ? "Record" : "Unassigned record"}
                </span>
                <span {...stylex.props(styles.columnName)}>
                  <Ref kind="observation" id={observation.id} label={recordName(observation)} />
                </span>
                <span {...stylex.props(styles.columnMeta)}>
                  <Source name={observation.source} />
                  <Code>{named.get(observation.id)}</Code>
                </span>
              </ShelfTable.Head>
            ))}
            {candidates.map((candidate) => (
              <ShelfTable.Head key={candidate.identity.id} scope="col" style={[styles.cell, styles.column]}>
                <span {...stylex.props(styles.columnKind)}>Matched identity</span>
                <span {...stylex.props(styles.columnName)}>
                  <Ref kind="identity" id={candidate.identity.id} label={identityName(candidate.identity)} />
                </span>
                <span {...stylex.props(styles.columnMeta)}>
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
            <ShelfTable.Row key={field} data-field={field} style={styles.row}>
              <ShelfTable.Head scope="row" style={[styles.cell, styles.fieldName]}>
                {field}
              </ShelfTable.Head>
              {observations.map((observation) => (
                <ShelfTable.Cell key={observation.id} style={[styles.cell, styles.value, styles.waits]}>
                  <Value value={valueOf(observation, field)} />
                </ShelfTable.Cell>
              ))}
              {candidates.map((candidate, index) => {
                const held = said[index]!.find((fact) => fact.field === field)?.values ?? [];
                const baseline = first ? valueOf(first, field) : null;
                // A value the record that waits holds too comes first, so that it is among those written out.
                const agreeing = held.filter((value) => baseline !== null && same(value.value, baseline));
                const ordered = [...agreeing, ...held.filter((value) => !agreeing.includes(value))];
                return (
                  <ShelfTable.Cell key={candidate.identity.id} style={[styles.cell, styles.value]}>
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
                      <span data-part="more" {...stylex.props(styles.none)}>
                        +{ordered.length - MOST_VALUES} more
                      </span>
                    ) : null}
                  </ShelfTable.Cell>
                );
              })}
            </ShelfTable.Row>
          ))}
          <ShelfTable.Row data-part="found" style={styles.row}>
            <ShelfTable.Head scope="row" style={[styles.cell, styles.rowName]}>
              Judgements
            </ShelfTable.Head>
            {observations.map((observation) => (
              <ShelfTable.Cell key={observation.id} style={[styles.cell, styles.value, styles.waits]}>
                <Judgements
                  judgements={judgements.filter(
                    (judgement) =>
                      judgement.observation === observation.id &&
                      observations.some((other) => other.id === judgement.other),
                  )}
                  named={named}
                />
              </ShelfTable.Cell>
            ))}
            {candidates.map((candidate) => (
              <ShelfTable.Cell key={candidate.identity.id} style={[styles.cell, styles.value]}>
                {/* Against any of its records: the case does not hold them all, so the judgement says whose it is. */}
                <Judgements
                  judgements={judgements.filter(
                    (judgement) =>
                      judgement.identity === candidate.identity.id &&
                      !observations.some((other) => other.id === judgement.other),
                  )}
                  named={named}
                />
              </ShelfTable.Cell>
            ))}
          </ShelfTable.Row>
          {constraints.length > 0 ? (
            <ShelfTable.Row data-part="decided" style={styles.row}>
              <ShelfTable.Head scope="row" style={[styles.cell, styles.rowName]}>
                Constraints
              </ShelfTable.Head>
              {observations.map((observation) => (
                <ShelfTable.Cell key={observation.id} style={[styles.cell, styles.value, styles.waits]}>
                  {/* Under a record: what was decided about it and a record of none of the identities. */}
                  <Constraints
                    constraints={constraints.filter(
                      ({ a, b, identities }) =>
                        (a === observation.id || b === observation.id) &&
                        !identities.some((identity) => identity !== null && matched.has(identity)),
                    )}
                    names={names}
                  />
                </ShelfTable.Cell>
              ))}
              {candidates.map((candidate) => (
                <ShelfTable.Cell key={candidate.identity.id} style={[styles.cell, styles.value]}>
                  <Constraints
                    constraints={constraints.filter(({ identities }) => identities.includes(candidate.identity.id))}
                    names={names}
                  />
                </ShelfTable.Cell>
              ))}
            </ShelfTable.Row>
          ) : null}
          {open && waiting.length > 0 ? (
            <ShelfTable.Row style={[styles.row, styles.decide]}>
              <ShelfTable.Head scope="row" style={[styles.cell, styles.rowName]}>
                Assign
              </ShelfTable.Head>
              <ShelfTable.Cell colSpan={observations.length} style={[styles.cell, styles.waits]}>
                <Place
                  found={found}
                  waiting={waiting}
                  rules={rules}
                  to={NEW}
                  label={waiting.length === 1 ? "New identity" : "New identities"}
                  icon={<PlusIcon />}
                  done={done}
                />
              </ShelfTable.Cell>
              {candidates.map((candidate) => (
                <ShelfTable.Cell key={candidate.identity.id} style={styles.cell}>
                  {waiting.every((observation) => rules.barred(observation.id, candidate.identity.id)) ? (
                    <span {...stylex.props(styles.none)}>Blocked by a constraint</span>
                  ) : (
                    <Place
                      found={found}
                      waiting={waiting}
                      rules={rules}
                      to={candidate.identity.id}
                      label={`Assign to ${identityName(candidate.identity)}`}
                      icon={<CheckIcon />}
                      primary
                      done={done}
                    />
                  )}
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
function Value({ value, agrees, times }: { value: string | null; agrees?: boolean; times?: number }) {
  if (value === null) return <span {...stylex.props(styles.none)}>empty</span>;
  return (
    <span data-agrees={agrees} {...stylex.props(styles.held)}>
      {agrees === undefined ? null : agrees ? (
        <CheckIcon aria-label="Same" aria-hidden={false} {...stylex.props(styles.agrees)} />
      ) : (
        <span role="img" aria-label="Different" {...stylex.props(styles.differs)} />
      )}
      <span>
        {value}
        {times !== undefined ? <span {...stylex.props(styles.times)}> ×{times}</span> : null}
      </span>
    </span>
  );
}

/** What the constraints in force say of some pairs, each with the changeset that made it. */
function Constraints({ constraints, names }: { constraints: CaseConstraint[]; names: Map<string, string> }) {
  if (constraints.length === 0) return <span {...stylex.props(styles.none)}>none</span>;
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
            <Ref kind="observation" id={constraint.a} label={names.get(constraint.a)} /> and{" "}
            <Ref kind="observation" id={constraint.b} label={names.get(constraint.b)} />, since{" "}
            <Ref kind="log" id={constraint.from} label={`log position ${constraint.from}`} />
          </span>
          <Onward path={["explain"]} query={{ a: constraint.a, b: constraint.b }}>
            explain
          </Onward>
        </span>
      ))}
    </span>
  );
}

/** The rules a strategy named with a verdict, when what it kept says so. */
function rulesOf(detail: unknown, verdict: string): string[] {
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
function Judgements({ judgements, named }: { judgements: PairJudgement[]; named: Map<string, string> }) {
  const [all, setAll] = useState(false);
  const ordered = foremost(judgements);
  const shown = all ? ordered : ordered.slice(0, MOST_JUDGEMENTS);
  const more = ordered.slice(shown.length);
  // One that is against a record the case does not hold is named by looking the record up.
  const names = useNames(
    shown.map((judgement) => judgement.other),
    named,
  );
  if (judgements.length === 0) return <span {...stylex.props(styles.none)}>none</span>;
  return (
    <span {...stylex.props(styles.judgements)}>
      {shown.map((judgement) => {
        const by = rulesOf(judgement.detail, judgement.verdict);
        return (
          <span
            key={`${judgement.observation}-${judgement.other}`}
            data-part="judgement"
            {...stylex.props(styles.judgement)}
          >
            <Word value={judgement.verdict} />
            <span>
              with <Ref kind="observation" id={judgement.other} label={names.get(judgement.other)} />
              {by.length > 0 ? ` by ${by.join(", ")}` : ""}
              {judgement.score !== null && judgement.score !== undefined ? `, score ${judgement.score}` : ""}
            </span>
            <Onward path={["explain"]} query={{ a: judgement.observation, b: judgement.other }}>
              explain
            </Onward>
          </span>
        );
      })}
      {more.length > 0 ? (
        <span {...stylex.props(styles.judgement)}>
          <span data-part="more" {...stylex.props(styles.none)}>
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

/**
 * Places the records that wait. The button says where it would put them,
 * and the dialog lets each go somewhere else before the change is made.
 */
function Place({
  found,
  waiting,
  rules,
  to,
  label,
  icon,
  primary = false,
  done,
}: {
  found: ReviewCase;
  waiting: ObservationRecord[];
  rules: Ruled;
  /** Where the dialog proposes to put them: an identity, or `NEW`. */
  to: string;
  label: string;
  icon: ReactNode;
  primary?: boolean;
  done: (said: ReactNode) => void;
}) {
  const { item, candidates } = found;
  return (
    <Act
      label={label}
      icon={icon}
      size="sm"
      variant={primary ? "default" : "outline"}
      title={waiting.length === 1 ? `Assign ${recordName(waiting[0]!)}` : `Assign ${count(waiting.length, "record")}`}
      who
      why="Reason"
      act={async (client, { actor, reason, entered }) => {
        const chosen = waiting.flatMap((observation) => {
          const where = String(entered.get(`to-${observation.id}`) ?? "");
          return where === "" ? [] : [{ observation: observation.id, to: where }];
        });
        if (chosen.length === 0) throw new Mistake("Choose an identity for at least one record.");
        const created = chosen.filter((choice) => choice.to === NEW).length;
        const minted = created > 0 ? (await client.mintIds({ identities: created })).identities : [];
        const ops: Op[] = [];
        for (const choice of chosen) {
          let identity = choice.to;
          if (choice.to === NEW) {
            identity = minted.shift()!;
            ops.push({ op: "create_identity", entity_type: item.entity_type, identity });
          }
          ops.push({ op: "assign", observation: choice.observation, from: null, to: identity });
        }
        const committed = await client.commit({ actor, reason, ops });
        return (
          <>
            {count(chosen.length, "record")} assigned at <At committed={committed} />.
          </>
        );
      }}
      done={done}
    >
      {waiting.map((observation) => (
        <Labelled key={observation.id} label={observationName(observation)}>
          {/* An identity the record is kept apart from is not a choice, and is not proposed. */}
          <Select
            name={`to-${observation.id}`}
            defaultValue={rules.barred(observation.id, to) ? "" : to}
            any="Leave unassigned"
            choices={[
              ...candidates
                .filter(({ identity }) => !rules.barred(observation.id, identity.id))
                .map(({ identity }) => ({
                  value: identity.id,
                  label: `${identityName(identity)} (${short(identity.id)})`,
                })),
              { value: NEW, label: "New identity" },
            ]}
          />
        </Labelled>
      ))}
    </Act>
  );
}

/**
 * Says that the identities the records matched are one: merges them into
 * the one with the most records, and places the records that wait in it,
 * as one changeset. It is written against where the log ended before the
 * case was read, so the engine refuses it if those identities changed
 * since.
 */
function MergeAndPlace({
  found,
  head,
  waiting,
  done,
}: {
  found: ReviewCase;
  head: number;
  waiting: ObservationRecord[];
  done: (said: ReactNode) => void;
}) {
  const [into, ...others] = [...found.candidates].sort(
    (a: Candidate, b: Candidate) => b.identity.members - a.identity.members,
  );
  if (!into) return null;
  return (
    <Act
      label="Merge identities"
      icon={<MergeIcon />}
      title={`Merge ${count(found.candidates.length, "identity")}`}
      about={`Merges ${others.map(({ identity }) => identityName(identity)).join(", ")} into ${identityName(into.identity)}, and assigns ${waiting.length === 1 ? recordName(waiting[0]!) : count(waiting.length, "record")} to it.`}
      who
      why="Reason"
      act={async (client, { actor, reason }) => {
        const committed = await client.commit({
          actor,
          reason,
          base: head,
          ops: [
            ...others.map(({ identity }): Op => ({ op: "merge", from: identity.id, into: into.identity.id })),
            ...waiting.map((observation): Op => ({
              op: "assign",
              observation: observation.id,
              from: null,
              to: into.identity.id,
            })),
          ],
        });
        return (
          <>
            {count(found.candidates.length, "identity")} merged and {count(waiting.length, "record")} assigned at{" "}
            <At committed={committed} />.
          </>
        );
      }}
      done={done}
    />
  );
}

const styles = stylex.create({
  narrow: {
    gap: spacing["4"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "space-between",
  },
  nothing: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "dashed",
    borderWidth: 1,
    gap: spacing["1.5"],
    paddingBlock: "3rem",
    paddingInline: spacing["6"],
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    textAlign: "center",
  },
  nothingIcon: {
    color: colors.mutedForeground,
    fontSize: "1.5rem",
    marginBottom: spacing["2"],
  },
  more: {
    display: "flex",
    justifyContent: "center",
  },
  card: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    backgroundColor: colors.background,
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
    overflow: "hidden",
  },
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
  apart: {
    color: colors.mutedForeground,
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    marginInlineEnd: "auto",
  },
  cardFoot: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    gap: spacing["2"],
    paddingBlock: spacing["2.5"],
    paddingInline: spacing["4"],
    alignItems: "center",
    backgroundColor: colors.card,
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "flex-end",
  },
  compare: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
  },
  row: {
    borderBottomWidth: { default: 1, ":last-child": 0 },
  },
  cell: {
    borderInlineStartColor: colors.border,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: { default: 1, ":first-child": 0 },
    paddingBlock: spacing["2"],
    paddingInline: spacing["4"],
    height: "auto",
    verticalAlign: "top",
    whiteSpace: "normal",
  },
  corner: {
    width: "1%",
  },
  column: {
    color: colors.foreground,
    fontWeight: typography.fontWeightRegular,
    minWidth: "13rem",
    paddingBlock: spacing["3"],
  },
  columnKind: {
    color: colors.mutedForeground,
    display: "block",
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
  },
  columnName: {
    display: "block",
    fontSize: typography.fontSizeBase,
    fontWeight: typography.fontWeightSemibold,
    lineHeight: typography.lineHeightBase,
    overflowWrap: "anywhere",
  },
  columnMeta: {
    gap: spacing["2"],
    alignItems: "center",
    color: colors.mutedForeground,
    display: "flex",
    flexWrap: "wrap",
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
    marginTop: spacing["1.5"],
  },
  waits: {
    backgroundColor: colors.card,
  },
  fieldName: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
    fontWeight: typography.fontWeightRegular,
    whiteSpace: "nowrap",
    width: "1%",
  },
  rowName: {
    color: colors.mutedForeground,
    fontSize: typography.fontSizeXs,
    fontWeight: typography.fontWeightMedium,
    verticalAlign: "top",
    whiteSpace: "nowrap",
    width: "1%",
  },
  value: {
    overflowWrap: "anywhere",
  },
  none: {
    color: colors.mutedForeground,
    fontStyle: "italic",
  },
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
  times: {
    color: colors.mutedForeground,
    fontVariantNumeric: "tabular-nums",
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
  decide: {
    backgroundColor: colors.card,
  },
});
