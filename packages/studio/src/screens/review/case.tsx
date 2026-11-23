// One review item: what is read of it, and the card it is decided from.

import * as stylex from "@stylexjs/stylex";
import { getRouteApi } from "@tanstack/react-router";
import { useState } from "react";
import type { ReactNode } from "react";

import { Done } from "../../act.tsx";
import type { RecordEntry, ReviewCase, ReviewEntry } from "../../api.ts";
import { Field, Fields, Load, Page, Reason } from "../../components/layout.tsx";
import { Ref, When, Word } from "../../components/refs.tsx";
import { InboxIcon } from "../../icons.tsx";
import { keys, useRead } from "../../queries.ts";
import { Who } from "../../who.tsx";
import { shared } from "./shared.ts";
import { CardHead, namer, theRecords } from "./card.tsx";
import { SideBySide } from "./sides.tsx";
import { Answers, ruled } from "./answers.tsx";
import { Compare } from "./compare.tsx";
import { SplitCard } from "./split.tsx";

const one = getRouteApi("/review/$id");

export function ReviewItem() {
  const { id } = one.useParams();
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
export function Case({
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
  const loaded = useRead(keys.review.one(id), async (client) => {
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
    <Fields>
      <Field label="Kind">
        <Word value={item.kind} />
      </Field>
      <Field label="Status">
        <Word value={item.status} />
      </Field>
      <Field label="Entity type">{item.entity_type}</Field>
      <Field label="Run">
        <Ref kind="run" id={item.run} label={`${item.kind === "possible_split" ? "" : "reconcile "}run ${item.run}`} />
      </Field>
      <Field label="Resolved at">
        {item.resolved_seq ? (
          <Ref kind="log" id={item.resolved_seq} label={`log position ${item.resolved_seq}`} />
        ) : null}
      </Field>
      <Field label="Superseded by">
        {item.superseded_by ? (
          <Ref
            kind="run"
            id={item.superseded_by}
            label={`${item.kind === "possible_split" ? "" : "reconcile "}run ${item.superseded_by}`}
          />
        ) : null}
      </Field>
      <Field label="Dismissed by">
        {item.dismissed ? (
          <>
            <Who actor={item.dismissed.by} /> at <When at={item.dismissed.at} />
            {item.dismissed.reason ? (
              <>
                {" "}
                <Reason>{item.dismissed.reason}</Reason>
              </>
            ) : null}
          </>
        ) : null}
      </Field>
    </Fields>
  );
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
    <article aria-label={`Review item ${item.id}`} {...stylex.props(shared.card)}>
      <CardHead found={found} linked={linked}>
        {open ? <Lead found={found} waiting={waiting} /> : null}
      </CardHead>
      {candidates.length > 0 ? <SideBySide found={found} /> : null}
      {open ? <Answers found={found} head={head} waiting={waiting} rules={rules} done={done} /> : null}
      <section aria-label="Field by field" {...stylex.props(shared.compare)}>
        <h3 {...stylex.props(shared.partTitle)}>Field by field</h3>
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
    <p data-part="lead" {...stylex.props(shared.why)}>
      {item.kind === "possible_merge"
        ? `The latest match run judged ${what} a match with records of each of these identities. A match run never merges identities, so placing ${one ? "it" : "them"} in one would be a guess and in all of them would merge them: it left the question to a person. `
        : `The latest match run judged ${what} a match${candidates.length === 1 ? ` with records of ${call(candidates[0]!.identity)}` : ""}, and ${against ? "a no_match rule held against " + (candidates.length === 1 ? "others of its records" : "some of the pairs") : "the evidence does not agree"}. It does not place records against a no_match: it left the question to a person. `}
      {waiting.length === 0
        ? "None of its records waits any more."
        : `Until someone answers, ${it} ${one ? "has" : "have"} no identity, and every match run of ${item.entity_type} looks at ${one ? "it" : "them"} again.`}
    </p>
  );
}
