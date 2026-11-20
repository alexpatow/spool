import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done, Mistake } from "../act.tsx";
import type {
  Candidate,
  CaseConstraint,
  IdentitySummary,
  RecordEntry,
  Op,
  PairJudgement,
  ReviewCase,
  ReviewEntry,
  ReviewStatus,
} from "../api.ts";
import {
  Choose,
  Code,
  Fields,
  Filters,
  Flag,
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
  sortRows,
  When,
  Word,
} from "../components.tsx";
import { Button } from "spool-ui/components/button";
import * as ShelfTable from "spool-ui/components/table";
import { facts, fieldsOf, latest, same, sourcesOf, valueOf } from "../facts.ts";
import { count, recordName, short, tail } from "../format.ts";
import { useLoad, useNames, useStudio } from "../hooks.ts";
import { CheckIcon, InboxIcon, MergeIcon, PlusIcon, SplitIcon } from "../icons.tsx";
import { href, navigate } from "../router.ts";
import type { Route } from "../router.ts";
import { colors, display, linen, radius, spacing, typography } from "spool-ui/tokens.stylex";
import { tones } from "../styles/tones.stylex";
import { foremost, tally } from "../verdicts.ts";
import { identityName } from "./identities.tsx";
import { Who } from "../who.tsx";

const STATUSES: ReviewStatus[] = ["open", "resolved", "dismissed", "superseded"];

/** How many items are shown with all a reviewer decides them from, before the reader asks for more. */
const AT_ONCE = 10;

/** The orders the items can be read in. The server gives them oldest first. */
const ORDERS = [
  { value: "oldest", label: "oldest first" },
  { value: "newest", label: "newest first" },
  { value: "records", label: "most records first" },
  { value: "kind", label: "kind" },
];

function ordered(items: ReviewEntry[], order: string): ReviewEntry[] {
  switch (order) {
    case "newest":
      return sortRows(items, (item) => item.id, "desc");
    case "records":
      return sortRows(items, (item) => item.records.length, "desc");
    case "kind":
      return sortRows(items, (item) => item.kind, "asc");
    default:
      return items;
  }
}

export function ReviewItems({ route, types }: { route: Route; types: string[] }) {
  const { type } = route.query;
  const status = route.query.status ?? "open";
  const order = route.query.sort ?? "oldest";
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
        <div {...stylex.props(styles.listing)}>
          {types.length > 1 ? (
            <Filters
              path={["review"]}
              query={route.query}
              selects={[{ name: "type", label: "Entity type", choices: types.map((value) => ({ value })) }]}
            />
          ) : null}
          <Labelled label="Order">
            <Select
              value={order}
              onChange={(value) =>
                navigate(href(["review"], { ...route.query, sort: value === "oldest" ? undefined : value }))
              }
              choices={ORDERS}
            />
          </Labelled>
        </div>
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
              {ordered(items, order)
                .slice(0, shown)
                .map((item) => (
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
          {found.item.kind === "possible_split" ? (
            <SplitCard found={found} head={head} linked={linked} done={done} />
          ) : (
            <CaseCard found={found} head={head} linked={linked} done={done} />
          )}
        </>
      )}
    </Load>
  );
}

function Details({ item }: { item: ReviewEntry }) {
  return (
    <Fields
      rows={[
        ["Kind", <Word value={item.kind} />],
        ["Status", <Word value={item.status} />],
        ["Entity type", item.entity_type],
        [
          "Run",
          <Ref
            kind="run"
            id={item.run}
            label={`${item.kind === "possible_split" ? "" : "reconcile "}run ${item.run}`}
          />,
        ],
        [
          "Resolved at",
          item.resolved_seq ? (
            <Ref kind="log" id={item.resolved_seq} label={`log position ${item.resolved_seq}`} />
          ) : null,
        ],
        [
          "Superseded by",
          item.superseded_by ? (
            <Ref
              kind="run"
              id={item.superseded_by}
              label={`${item.kind === "possible_split" ? "" : "reconcile "}run ${item.superseded_by}`}
            />
          ) : null,
        ],
        [
          "Dismissed by",
          item.dismissed ? (
            <>
              <Who actor={item.dismissed.by} /> at <When at={item.dismissed.at} />
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
function labelOf(record: RecordEntry): string {
  return record.label ?? recordName(record);
}

/**
 * What the constraints in force rule out for a case. The engine refuses a
 * changeset that would break one, so the screen does not offer it.
 */
interface Ruled {
  /** Whether a record may not be placed in an identity: a member of it must not share an identity with it. */
  barred: (record: string, identity: string) => boolean;
  /** Whether the identities cannot be merged with the records that wait placed in the one that stands. */
  apart: boolean;
}

function ruled({ candidates, constraints }: ReviewCase, waiting: RecordEntry[]): Ruled {
  const matched = new Set(candidates.map((candidate) => candidate.identity.id));
  const waits = new Set(waiting.map((record) => record.id));
  const kept = constraints.filter((constraint) => constraint.kind === "cannot_link");
  // After the merge, a record is in the one identity if it waits or belongs to one that was matched.
  const joined = (record: string, identity: string | null) =>
    waits.has(record) || (identity !== null && matched.has(identity));
  return {
    barred: (record, identity) =>
      kept.some(
        ({ a, b, identities }) =>
          (a === record && identities[1] === identity) || (b === record && identities[0] === identity),
      ),
    apart: kept.some(({ a, b, identities }) => joined(a, identities[0]) && joined(b, identities[1])),
  };
}

/**
 * What to call each of the identities a case names, where they are named
 * together: two that are called the same are told apart by the end of
 * their IDs.
 */
function namer(candidates: Candidate[]): (identity: IdentitySummary) => string {
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
function theRecords(records: RecordEntry[]): string {
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
  const { item, records, candidates } = found;
  const open = item.status === "open";
  // A record its source took back does not wait: the engine refuses it an identity.
  const waiting = records.filter((record) => !record.identity && record.retracted == null);
  const rules = ruled(found, waiting);
  return (
    <article aria-label={`Review item ${item.id}`} {...stylex.props(styles.card)}>
      <CardHead found={found} linked={linked}>
        {open ? <Lead found={found} waiting={waiting} /> : null}
      </CardHead>
      {candidates.length > 0 ? <SideBySide found={found} /> : null}
      {open ? <Answers found={found} head={head} waiting={waiting} rules={rules} done={done} /> : null}
      <section aria-label="Field by field" {...stylex.props(styles.compare)}>
        <h3 {...stylex.props(styles.partTitle)}>Field by field</h3>
        <Compare found={found} />
      </section>
    </article>
  );
}

/** Why the item waits for a person, and what waits meanwhile. */
function Lead({ found, waiting }: { found: ReviewCase; waiting: RecordEntry[] }) {
  const { item, candidates, judgements } = found;
  const what = theRecords(waiting.length > 0 ? waiting : found.records);
  const one = (waiting.length > 0 ? waiting : found.records).length === 1;
  const it = one ? "it" : "they";
  const against = judgements.some((judgement) => judgement.verdict === "no_match");
  const call = namer(candidates);
  return (
    <p data-part="lead" {...stylex.props(styles.why)}>
      {item.kind === "possible_merge"
        ? `The latest match run judged ${what} a match with records of each of these identities. A match run never merges identities, so placing ${one ? "it" : "them"} in one would be a guess and in all of them would merge them: it left the question to a person. `
        : `The latest match run judged ${what} a match${candidates.length === 1 ? ` with records of ${call(candidates[0]!.identity)}` : ""}, and ${against ? "a no_match rule held against " + (candidates.length === 1 ? "others of its records" : "some of the pairs") : "the evidence does not agree"}. It does not place records against a no_match: it left the question to a person. `}
      {waiting.length === 0
        ? "None of its records waits any more."
        : `Until someone answers, ${it} ${one ? "has" : "have"} no identity, and every match run of ${item.entity_type} looks at ${one ? "it" : "them"} again.`}
    </p>
  );
}

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
function SideBySide({ found }: { found: ReviewCase }) {
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
            <span {...stylex.props(styles.columnKind)}>Matched identity</span>
            <span {...stylex.props(styles.columnName)}>
              <Ref kind="identity" id={identity.id} label={call(identity)} />
            </span>
            <span data-part="size" {...stylex.props(styles.columnMeta)}>
              {count(identity.members, "record")}
              <Sources sources={identity.sources} />
            </span>
            <ul data-part="members" aria-label={`Some records of ${call(identity)}`} {...stylex.props(styles.sample)}>
              {members.slice(0, SAMPLE).map((record) => (
                <li key={record.id}>
                  <Ref kind="record" id={record.id} label={labelOf(record)} />{" "}
                  <span {...stylex.props(styles.times)}>{recordName(record)}</span>
                </li>
              ))}
              {identity.members > SAMPLE ? (
                <li {...stylex.props(styles.none)}>and {count(identity.members - Math.min(SAMPLE, members.length), "more record")}</li>
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

/** One answer to an item: what it is called, what it does, and the button that gives it. */
function Answer({
  name,
  title,
  does,
  action,
}: {
  name: string;
  title: string;
  does: ReactNode[];
  action?: ReactNode;
}) {
  return (
    <li data-part="answer" data-answer={name} {...stylex.props(styles.answer)}>
      <div {...stylex.props(styles.answerText)}>
        <h4 {...stylex.props(styles.answerTitle)}>{title}</h4>
        <ul {...stylex.props(styles.does)}>
          {does.map((said, index) => (
            <li key={index}>{said}</li>
          ))}
        </ul>
      </div>
      {action ? <div {...stylex.props(styles.answerAction)}>{action}</div> : null}
    </li>
  );
}

/** What undoing a placement from the log does. */
const UNDO_WAITS = (
  <>
    Undo it from the log and the records wait again with no identity. No match run asks about them then, only a
    rematch (<Code>spool match --rematch</Code>).
  </>
);

/**
 * Each answer the item can be given, with what it does before it is given:
 * where the records go, what becomes of the identities and their IDs, what
 * is recorded, what later runs do, and what undoing it puts back.
 */
function Answers({
  found,
  head,
  waiting,
  rules,
  done,
}: {
  found: ReviewCase;
  head: number;
  waiting: RecordEntry[];
  rules: Ruled;
  done: (said: ReactNode) => void;
}) {
  const { item, candidates } = found;
  const n = waiting.length;
  const these = n === 1 ? labelOf(waiting[0]!) : `The ${count(n, "record")}`;
  const those = n === 1 ? labelOf(waiting[0]!) : `the ${count(n, "record")}`;
  const them = n === 1 ? "it" : "them";
  const call = namer(candidates);
  const names = candidates.map((candidate) => call(candidate.identity));
  const [into, ...others] = [...candidates].sort((a, b) => b.identity.members - a.identity.members);
  const dismiss = (
    <Act
      label="Dismiss"
      variant="ghost"
      title={`Dismiss review item ${item.id}`}
      about="Its records keep no identity, and no later match run looks at them again."
      why="Reason"
      act={async (client, { reason }) => {
        await client.dismissReview(item.id, { reason });
        return "Dismissed.";
      }}
      done={done}
    />
  );
  if (n === 0) {
    return (
      <section aria-label="Answers" {...stylex.props(styles.answers)}>
        <h3 {...stylex.props(styles.partTitle)}>Answers</h3>
        <ol {...stylex.props(styles.answerList)}>
          <Answer
            name="dismiss"
            title="Close it"
            does={["None of its records waits for an identity any more, so there is nothing to place. Dismissing it changes no identity."]}
            action={dismiss}
          />
        </ol>
      </section>
    );
  }
  return (
    <section aria-label="Answers" {...stylex.props(styles.answers)}>
      <h3 {...stylex.props(styles.partTitle)}>What each answer does</h3>
      <ol {...stylex.props(styles.answerList)}>
        {into && others.length > 0 ? (
          <Answer
            name="merge"
            title={`${candidates.length === 2 ? "They are" : "All of them are"} the same thing: merge them`}
            does={
              rules.apart
                ? [
                    "Not possible: a cannot_link constraint is between records of these identities, so a changeset that merges them is refused. The field by field comparison below names it.",
                  ]
                : [
                    `${listed(others.map(({ identity }) => call(identity)))} ${others.length === 1 ? "is" : "are"} merged into ${call(into.identity)}, which ${others.every(({ identity }) => identity.members < into.identity.members) ? "has the most records" : "has as many records as any and comes first"} and keeps its ID. ${others.length === 1 ? "The other ID" : "The other IDs"} then ${others.length === 1 ? "answers" : "answer"} with ${call(into.identity)}'s, so links to ${others.length === 1 ? "it" : "them"} keep working.`,
                    `${these} ${n === 1 ? "is" : "are"} placed in it too: ${call(into.identity)} then has ${count(candidates.reduce((sum, { identity }) => sum + identity.members, 0) + n, "record")}.`,
                    "No constraint is recorded. A match run never splits an identity, so they stay together until a person splits them.",
                    `Undo it from the log and ${listed(others.map(({ identity }) => call(identity)))} ${others.length === 1 ? "gets its ID and its records" : "get their IDs and their records"} back, and ${those} ${n === 1 ? "waits" : "wait"} again. No match run asks about ${them} then, only a rematch.`,
                  ]
            }
            action={rules.apart ? undefined : <MergeAndPlace found={found} head={head} waiting={waiting} done={done} />}
          />
        ) : null}
        {candidates.length > 0 ? (
          <Answer
            name="assign"
            title={
              candidates.length === 1
                ? `${n === 1 ? "It belongs" : "They belong"} in ${names[0]}`
                : `${n === 1 ? "It belongs" : "They belong"} in one of them, and the identities are different things`
            }
            does={[
              candidates.length === 1
                ? `${these} ${n === 1 ? "is" : "are"} placed in ${names[0]}, which then has ${count(candidates[0]!.identity.members + n, "record")}. Its ID stays as it is.`
                : `${these} ${n === 1 ? "is" : "are"} placed in the one you choose, which then has ${listed(candidates.map(({ identity }) => `${count(identity.members + n, "record")} for ${call(identity)}`))}. The others stay as they are, and every ID stays.`,
              candidates.length > 1
                ? "Nothing is recorded that keeps the identities apart: a later record that matches more than one of them is asked about again. A match run never moves a record that has an identity."
                : "A match run never moves a record that has an identity.",
              UNDO_WAITS,
              ...(n > 1 ? ["Before it is made, the dialog lets each record go somewhere else, or stay waiting."] : []),
              ...candidates
                .filter((candidate) => waiting.every((record) => rules.barred(record.id, candidate.identity.id)))
                .map(
                  (candidate) =>
                    `Not ${call(candidate.identity)}: a cannot_link keeps ${them} apart from one of its records.`,
                ),
            ]}
            action={
              <div {...stylex.props(styles.choices)}>
                {candidates
                  .filter((candidate) => !waiting.every((record) => rules.barred(record.id, candidate.identity.id)))
                  .map((candidate) => (
                    <Place
                      key={candidate.identity.id}
                      found={found}
                      waiting={waiting}
                      rules={rules}
                      to={candidate.identity.id}
                      label={`Assign to ${call(candidate.identity)}`}
                      icon={<CheckIcon />}
                      primary
                      done={done}
                    />
                  ))}
              </div>
            }
          />
        ) : null}
        <Answer
          name="new"
          title={`${n === 1 ? "It is" : "They are"} none of ${candidates.length === 0 ? "the identities there are" : listed(names)}`}
          does={[
            n === 1
              ? `${these} becomes an identity of its own, with an ID made when the change is made.`
              : `Each of the ${count(n, "record")} becomes an identity of its own, with an ID made when the change is made. The dialog lets some of them go elsewhere instead.`,
            candidates.length > 0 ? `${listed(names)} ${candidates.length === 1 ? "stays as it is" : "stay as they are"}.` : null,
            "Undo it from the log and the records wait again with no identity, and only a rematch asks about them. An identity that was made is not taken back: its ID stays, with no records.",
          ].filter((said): said is string => said !== null)}
          action={
            <Place
              found={found}
              waiting={waiting}
              rules={rules}
              to={NEW}
              label={n === 1 ? "New identity" : "New identities"}
              icon={<PlusIcon />}
              done={done}
            />
          }
        />
        <Answer
          name="dismiss"
          title="Not sure, or not one to keep: dismiss it"
          does={[
            `${these} ${n === 1 ? "keeps" : "keep"} no identity, and no later match run looks at ${them} again, a rematch included.`,
            `A dismissed item is not opened again. A changeset can still place ${them}, from the screen of each record.`,
          ]}
          action={dismiss}
        />
        <Answer
          name="leave"
          title="Leave it open"
          does={[
            `Nothing changes. Every match run of ${item.entity_type} looks at ${them} again: if the evidence has changed by then, it places ${them} or replaces this item with another.`,
          ]}
        />
      </ol>
      <div {...stylex.props(styles.checked)}>
        <Quiet>
          A merge is refused, changing nothing, if the records of these identities changed since this screen was read.
          A placement is refused if one of its records has an identity by then. Neither is checked for anything else
          that changed since.
        </Quiet>
      </div>
    </section>
  );
}

/** What every card starts with: which item it is, what it asks, and what became of it. */
function CardHead({ found, linked, children }: { found: ReviewCase; linked: boolean; children?: ReactNode }) {
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

/** In a new identity, as a choice of where a record goes. */
const NEW = "new";

/** The most values of one field that a card writes out for an identity. The identity's own screen has them all. */
const MOST_VALUES = 4;

/**
 * The records an item names beside the identities they matched, a field to
 * a row, so that what agrees and what does not is read across. Under each
 * column is the decision that column stands for.
 */
function Compare({ found }: { found: ReviewCase }) {
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
            {records.map((record) => (
              <ShelfTable.Head key={record.id} scope="col" style={[styles.cell, styles.column, styles.waits]}>
                <span data-part="record-kind" {...stylex.props(styles.columnKind)}>
                  {record.identity ? "Record" : record.retracted != null ? "Taken back by its source" : "Unassigned record"}
                </span>
                <span {...stylex.props(styles.columnName)}>
                  <Ref kind="record" id={record.id} label={labelOf(record)} />
                </span>
                <span {...stylex.props(styles.columnMeta)}>
                  <Source name={record.source} />
                  <Code>{named.get(record.id)}</Code>
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
              {records.map((record) => (
                <ShelfTable.Cell key={record.id} style={[styles.cell, styles.value, styles.waits]}>
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
            {records.map((record) => (
              <ShelfTable.Cell key={record.id} style={[styles.cell, styles.value, styles.waits]}>
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
              <ShelfTable.Cell key={candidate.identity.id} style={[styles.cell, styles.value]}>
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
            <ShelfTable.Row data-part="decided" style={styles.row}>
              <ShelfTable.Head scope="row" style={[styles.cell, styles.rowName]}>
                Constraints
              </ShelfTable.Head>
              {records.map((record) => (
                <ShelfTable.Cell key={record.id} style={[styles.cell, styles.value, styles.waits]}>
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
                <ShelfTable.Cell key={candidate.identity.id} style={[styles.cell, styles.value]}>
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
            <Ref kind="record" id={constraint.a} label={names.get(constraint.a)} /> and{" "}
            <Ref kind="record" id={constraint.b} label={names.get(constraint.b)} />, since{" "}
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
            <Onward path={["explain"]} query={{ a: judgement.record, b: judgement.other }}>
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
  waiting: RecordEntry[];
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
      title={waiting.length === 1 ? `Assign ${labelOf(waiting[0]!)}` : `Assign ${count(waiting.length, "record")}`}
      why="Reason"
      act={async (client, { reason, entered }) => {
        const chosen = waiting.flatMap((record) => {
          const where = String(entered.get(`to-${record.id}`) ?? "");
          return where === "" ? [] : [{ record: record.id, to: where }];
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
          ops.push({ op: "assign", record: choice.record, from: null, to: identity });
        }
        const committed = await client.commit({ reason, ops });
        return (
          <>
            {count(chosen.length, "record")} assigned at <At committed={committed} />.
          </>
        );
      }}
      done={done}
    >
      {waiting.map((record) => (
        <Labelled key={record.id} label={recordName(record)}>
          {/* An identity the record is kept apart from is not a choice, and is not proposed. */}
          <Select
            name={`to-${record.id}`}
            defaultValue={rules.barred(record.id, to) ? "" : to}
            any="Leave unassigned"
            choices={[
              ...candidates
                .filter(({ identity }) => !rules.barred(record.id, identity.id))
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
  waiting: RecordEntry[];
  done: (said: ReactNode) => void;
}) {
  const [into, ...others] = [...found.candidates].sort(
    (a: Candidate, b: Candidate) => b.identity.members - a.identity.members,
  );
  if (!into) return null;
  const call = namer(found.candidates);
  return (
    <Act
      label="Merge identities"
      icon={<MergeIcon />}
      title={`Merge ${count(found.candidates.length, "identity")}`}
      about={`Merges ${others.map(({ identity }) => call(identity)).join(", ")} into ${call(into.identity)}, and assigns ${waiting.length === 1 ? recordName(waiting[0]!) : count(waiting.length, "record")} to it.`}
      why="Reason"
      act={async (client, { reason }) => {
        const committed = await client.commit({
          reason,
          base: head,
          ops: [
            ...others.map(({ identity }): Op => ({ op: "merge", from: identity.id, into: into.identity.id })),
            ...waiting.map((record): Op => ({
              op: "assign",
              record: record.id,
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
function SplitCard({
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
  const names = useNames(
    constraints.flatMap((constraint) => [constraint.a, constraint.b]),
    named,
  );
  const fields = fieldsOf([...records, ...takenBack]);
  const said = groups.map((group) => facts(latest(group)));
  const [apart, setApart] = useState(true);
  return (
    <article aria-label={`Review item ${item.id}`} {...stylex.props(styles.card)}>
      <CardHead found={found} linked={linked}>
        <p data-part="why" {...stylex.props(styles.why)}>
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
      <div {...stylex.props(styles.compare)}>
        <ShelfTable.Root aria-label={`Review item ${item.id}`}>
          <ShelfTable.Header>
            <ShelfTable.Row style={styles.row}>
              <ShelfTable.Head style={[styles.cell, styles.corner]} />
              {groups.map((group, index) => (
                <ShelfTable.Head key={index} scope="col" style={[styles.cell, styles.column]}>
                  <span data-part="record-kind" {...stylex.props(styles.columnKind)}>
                    Group {index + 1}
                    {still[index]!.length === 0 ? ", no longer in the identity" : ""}
                  </span>
                  {group.map((record) => (
                    <span key={record.id} {...stylex.props(styles.columnName)}>
                      <Ref kind="record" id={record.id} label={labelOf(record)} />
                    </span>
                  ))}
                  <span {...stylex.props(styles.columnMeta)}>
                    {count(group.length, "record")}
                    <Sources sources={sourcesOf(group)} />
                    {group.length === 1 ? <Code>{named.get(group[0]!.id)}</Code> : null}
                  </span>
                </ShelfTable.Head>
              ))}
              {takenBack.map((record) => (
                <ShelfTable.Head key={record.id} scope="col" style={[styles.cell, styles.column, styles.waits]}>
                  <span data-part="record-kind" {...stylex.props(styles.columnKind)}>
                    Taken back by its source
                  </span>
                  <span {...stylex.props(styles.columnName)}>
                    <Ref kind="record" id={record.id} label={labelOf(record)} />
                  </span>
                  <span {...stylex.props(styles.columnMeta)}>
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
              <ShelfTable.Row key={field} data-field={field} style={styles.row}>
                <ShelfTable.Head scope="row" style={[styles.cell, styles.fieldName]}>
                  {field}
                </ShelfTable.Head>
                {groups.map((_, index) => {
                  const held = said[index]!.find((fact) => fact.field === field)?.values ?? [];
                  return (
                    <ShelfTable.Cell key={index} style={[styles.cell, styles.value]}>
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
                        <span data-part="more" {...stylex.props(styles.none)}>
                          +{held.length - MOST_VALUES} more
                        </span>
                      ) : null}
                    </ShelfTable.Cell>
                  );
                })}
                {takenBack.map((record) => (
                  <ShelfTable.Cell key={record.id} style={[styles.cell, styles.value, styles.waits]}>
                    <Value value={valueOf(record, field)} />
                  </ShelfTable.Cell>
                ))}
              </ShelfTable.Row>
            ))}
            <ShelfTable.Row data-part="found" style={styles.row}>
              <ShelfTable.Head scope="row" style={[styles.cell, styles.rowName]}>
                Judgements across groups
              </ShelfTable.Head>
              {groups.map((_, index) => (
                <ShelfTable.Cell key={index} style={[styles.cell, styles.value]}>
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
                <ShelfTable.Cell key={record.id} style={[styles.cell, styles.value, styles.waits]}>
                  {/* Under a record that was taken back: what it was judged against the records that remain. */}
                  <Judgements
                    judgements={judgements.filter((judgement) => judgement.other === record.id).map(turned)}
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
                {groups.map((_, index) => (
                  <ShelfTable.Cell key={index} style={[styles.cell, styles.value]}>
                    <Constraints
                      constraints={constraints.filter(({ a, b }) => groupOf.get(a) === index || groupOf.get(b) === index)}
                      names={names}
                    />
                  </ShelfTable.Cell>
                ))}
                {takenBack.map((record) => (
                  <ShelfTable.Cell key={record.id} style={[styles.cell, styles.value, styles.waits]} />
                ))}
              </ShelfTable.Row>
            ) : null}
          </ShelfTable.Body>
        </ShelfTable.Root>
      </div>
      {open && identity ? (
        <section aria-label="Answers" {...stylex.props(styles.answers)}>
          <h3 {...stylex.props(styles.partTitle)}>What each answer does</h3>
          <ol {...stylex.props(styles.answerList)}>
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
          <div {...stylex.props(styles.checked)}>
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

const styles = stylex.create({
  narrow: {
    gap: spacing["4"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "space-between",
  },
  listing: {
    gap: spacing["3"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
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
  why: {
    margin: 0,
    color: colors.mutedForeground,
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    maxWidth: "46rem",
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
  partTitle: {
    margin: 0,
    paddingBlock: spacing["3"],
    paddingInline: spacing["4"],
    color: colors.foreground,
    fontFamily: display.fontFamily,
    fontSize: typography.fontSizeLg,
    fontVariationSettings: '"opsz" 24',
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: typography.lineHeightLg,
  },
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
  answers: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    gap: spacing["2"],
    paddingBottom: spacing["4"],
    backgroundColor: colors.card,
    display: "flex",
    flexDirection: "column",
  },
  answerList: {
    margin: 0,
    paddingInline: spacing["4"],
    gap: spacing["2"],
    display: "flex",
    flexDirection: "column",
    listStyleType: "none",
  },
  answer: {
    borderColor: colors.border,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["4"],
    padding: spacing["3"],
    alignItems: "flex-start",
    backgroundColor: colors.background,
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "space-between",
  },
  answerText: {
    gap: spacing["1"],
    display: "flex",
    flexBasis: "32rem",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: 0,
  },
  answerTitle: {
    margin: 0,
    fontSize: typography.fontSizeSm,
    fontWeight: typography.fontWeightMedium,
    lineHeight: typography.lineHeightSm,
  },
  does: {
    margin: 0,
    gap: spacing["1"],
    paddingInlineStart: spacing["4"],
    color: colors.mutedForeground,
    display: "flex",
    flexDirection: "column",
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    maxWidth: "52rem",
  },
  answerAction: {
    flexShrink: 0,
  },
  checked: {
    paddingInline: spacing["4"],
  },
  choices: {
    gap: spacing["2"],
    alignItems: "flex-end",
    display: "flex",
    flexDirection: "column",
  },
});
