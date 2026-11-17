import { useState } from "react";

import type { DocumentRecord, ObservationRecord, ReviewRecord } from "../api.ts";
import {
  Fields,
  Filters,
  Id,
  Json,
  Load,
  Page,
  PagedTable,
  Ref,
  Section,
  When,
  Word,
  message,
  usePair,
} from "../components.tsx";
import { findObservation, found } from "../find.ts";
import { excerpt, mentionOf, named, observationName } from "../format.ts";
import type { Excerpt, Mention } from "../format.ts";
import { useLoad, usePages, useStudio } from "../hooks.ts";
import { href } from "../router.ts";
import type { Route } from "../router.ts";

export function Observations({ route, types }: { route: Route; types: string[] }) {
  const { type, source, key, document, unassigned } = route.query;
  const pair = usePair();
  const pages = usePages(async (client, after) => {
    const page = await client.observations({
      entity_type: type,
      source,
      key,
      document,
      unassigned: unassigned === "true" ? true : undefined,
      after: after as string | undefined,
    });
    return { items: page.observations, next: page.next };
  });
  return (
    <Page
      title="Observations"
      about={
        document ? (
          <>
            The mentions an extractor found in document <Ref kind="document" id={document} />.
          </>
        ) : (
          "Rows as they were ingested, and mentions an extractor found, with the identity each belongs to now."
        )
      }
    >
      <Filters
        path={["observations"]}
        query={route.query}
        selects={[{ name: "type", label: "Entity type", choices: types.map((value) => ({ value })) }]}
        texts={[
          { name: "source", label: "Source" },
          { name: "key", label: "Key" },
        ]}
        flags={[{ name: "unassigned", label: "Without an identity" }]}
      />
      <PagedTable
        pages={pages}
        columns={[
          { title: "", cell: (observation) => pair.tick(observation.id) },
          {
            title: "Observation",
            cell: (observation) => <Ref kind="observation" id={observation.id} label={observationName(observation)} />,
          },
          { title: "ID", cell: (observation) => <Ref kind="observation" id={observation.id} /> },
          { title: "Entity type", cell: (observation) => observation.entity_type },
          {
            title: "Identity",
            cell: (observation) =>
              observation.identity ? (
                <Ref kind="identity" id={observation.identity} />
              ) : (
                <Word value="unassigned" label="none" />
              ),
          },
        ]}
        rowKey={(observation) => observation.id}
        empty="No observations."
      />
      {pair.explain}
    </Page>
  );
}

interface Loaded {
  observation: ObservationRecord;
  review: ReviewRecord[];
  /** The document a mention was found in, if the store still has it. */
  document: DocumentRecord | null;
}

export function Observation({ id }: { id: string }) {
  const loaded = useLoad(async (client): Promise<Loaded> => {
    const [observation, review] = await Promise.all([client.observation(id), client.reviewItems({ observation: id })]);
    const mention = mentionOf(observation);
    const document = mention ? await found(client.document(mention.document)) : null;
    return { observation, review, document };
  });
  return (
    <Page title="Observation" about={<Id value={id} />}>
      <Load state={loaded}>{(loaded) => <ObservationScreen {...loaded} />}</Load>
    </Page>
  );
}

function ObservationScreen({ observation, review, document }: Loaded) {
  const mention = mentionOf(observation);
  const around = mention && document?.text ? excerpt(document.text, mention.start, mention.end) : null;
  return (
    <>
      <Fields
        rows={[
          ["Entity type", observation.entity_type],
          ["Source", observation.source],
          ["Key", observation.key ? <code>{observation.key}</code> : null],
          [
            "Ingested",
            <>
              <When at={observation.ingested_at} /> by{" "}
              <Ref kind="run" id={observation.ingest_run} label={`run ${observation.ingest_run}`} />
            </>,
          ],
          [
            "Identity",
            observation.identity ? (
              <Ref kind="identity" id={observation.identity} />
            ) : (
              <Word value="unassigned" label="none" />
            ),
          ],
          ["Replaces", observation.supersedes ? <Ref kind="observation" id={observation.supersedes} /> : null],
          ["Replaced by", observation.superseded_by ? <Ref kind="observation" id={observation.superseded_by} /> : null],
          [
            "Review",
            review.length > 0
              ? review.map((item, index) => (
                  <span key={item.id}>
                    {index > 0 ? ", " : ""}
                    <Ref kind="review" id={item.id} label={`item ${item.id}`} /> <Word value={item.kind} />{" "}
                    <Word value={item.status} />
                  </span>
                ))
              : null,
          ],
        ]}
      />
      {mention && document ? <Found mention={mention} document={document} excerpt={around} /> : null}
      <Section title="Payload">
        <Json value={observation.payload} />
      </Section>
      {observation.evidence !== undefined && observation.evidence !== null ? (
        <Section title="Evidence">
          <Json value={observation.evidence} />
        </Section>
      ) : null}
      <Section title="Explain">
        <ExplainWith observation={observation} />
      </Section>
    </>
  );
}

function Found({
  mention,
  document,
  excerpt,
}: {
  mention: Mention;
  document: DocumentRecord;
  excerpt: Excerpt | null;
}) {
  return (
    <Section title="Where it was found">
      <p>
        In document{" "}
        <a href={href(["documents", document.id], { at: mention.start })}>{named(document.source, document.key)}</a>,
        characters {mention.start} to {mention.end}.
      </p>
      {excerpt ? (
        <blockquote className="excerpt">
          {excerpt.cutBefore ? "…" : ""}
          {excerpt.before}
          <mark>{excerpt.text}</mark>
          {excerpt.after}
          {excerpt.cutAfter ? "…" : ""}
        </blockquote>
      ) : null}
    </Section>
  );
}

/** Asks for the other observation of a pair, and goes to the explanation of the two. */
function ExplainWith({ observation }: { observation: ObservationRecord }) {
  const { client } = useStudio();
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="filters"
      onSubmit={(event) => {
        event.preventDefault();
        const reference = String(new FormData(event.currentTarget).get("other") ?? "").trim();
        if (reference === "") return;
        setBusy(true);
        setProblem(null);
        findObservation(client, reference).then(
          (other) => {
            setBusy(false);
            if (other === null) setProblem(`No observation is named ${reference}.`);
            else window.location.hash = href(["explain"], { a: observation.id, b: other.id });
          },
          (error: unknown) => {
            setBusy(false);
            setProblem(message(error));
          },
        );
      }}
    >
      <label>
        Why is it with another observation, or apart from it?
        <input name="other" placeholder="an ID, or source:key" spellCheck={false} autoComplete="off" size={40} />
      </label>
      <button type="submit" disabled={busy}>
        Explain
      </button>
      {problem ? (
        <p className="notice failure" role="alert">
          {problem}
        </p>
      ) : null}
    </form>
  );
}
