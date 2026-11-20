// The answers to a review item: what each would do, said before it is
// done, and the change it makes.

import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { Act, At, Mistake } from "../../act.tsx";
import type { Candidate, RecordEntry, Op, ReviewCase } from "../../api.ts";
import { Code, Quiet } from "../../components/layout.tsx";
import { Labelled, Select } from "../../components/forms.tsx";
import { count, recordName, short } from "../../format.ts";
import { CheckIcon, MergeIcon, PlusIcon } from "../../icons.tsx";
import { stale } from "../../queries.ts";
import { colors, spacing, typography } from "spool-ui/tokens.stylex";
import { identityName } from "../identities.tsx";
import { shared } from "./shared.ts";
import { labelOf, listed, namer } from "./card.tsx";

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

export function ruled({ candidates, constraints }: ReviewCase, waiting: RecordEntry[]): Ruled {
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

/** One answer to an item: what it is called, what it does, and the button that gives it. */
export function Answer({
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
    Undo it from the log and the records wait again with no identity. No match run asks about them then, only a rematch
    (<Code>spool match --rematch</Code>).
  </>
);

/**
 * Each answer the item can be given, with what it does before it is given:
 * where the records go, what becomes of the identities and their IDs, what
 * is recorded, what later runs do, and what undoing it puts back.
 */
export function Answers({
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
      stale={stale.store}
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
      <section aria-label="Answers" {...stylex.props(shared.answers)}>
        <h3 {...stylex.props(shared.partTitle)}>Answers</h3>
        <ol {...stylex.props(shared.answerList)}>
          <Answer
            name="dismiss"
            title="Close it"
            does={[
              "None of its records waits for an identity any more, so there is nothing to place. Dismissing it changes no identity.",
            ]}
            action={dismiss}
          />
        </ol>
      </section>
    );
  }
  return (
    <section aria-label="Answers" {...stylex.props(shared.answers)}>
      <h3 {...stylex.props(shared.partTitle)}>What each answer does</h3>
      <ol {...stylex.props(shared.answerList)}>
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
            candidates.length > 0
              ? `${listed(names)} ${candidates.length === 1 ? "stays as it is" : "stay as they are"}.`
              : null,
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
      <div {...stylex.props(shared.checked)}>
        <Quiet>
          A merge is refused, changing nothing, if the records of these identities changed since this screen was read. A
          placement is refused if one of its records has an identity by then. Neither is checked for anything else that
          changed since.
        </Quiet>
      </div>
    </section>
  );
}

/** In a new identity, as a choice of where a record goes. */
const NEW = "new";

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
      stale={stale.store}
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
      stale={stale.store}
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

const styles = stylex.create({
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
  choices: {
    gap: spacing["2"],
    alignItems: "flex-end",
    display: "flex",
    flexDirection: "column",
  },
});
