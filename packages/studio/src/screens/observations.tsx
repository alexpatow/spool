import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done, Mistake } from "../act.tsx";
import type { DocumentRecord, ObservationRecord, Op, ReviewRecord } from "../api.ts";
import {
  Code,
  Fields,
  Filters,
  Id,
  Identity,
  Json,
  Labelled,
  Load,
  message,
  Notice,
  Page,
  PagedTable,
  Ref,
  Row,
  Section,
  Select,
  Source,
  Text,
  usePair,
  When,
  Word,
} from "../components.tsx";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { findIdentity, findObservation, found } from "../find.ts";
import { excerpt, mentionOf, named, observationName } from "../format.ts";
import type { Excerpt, Mention } from "../format.ts";
import { useLabels, useLoad, usePages, useStudio } from "../hooks.ts";
import { RecordIcon } from "../icons.tsx";
import { href, navigate } from "../router.ts";
import type { Route } from "../router.ts";
import { colors, radius, spacing, typography } from "../styles/shelf/tokens.stylex";
import { tones } from "../styles/tones.stylex";

/** What a record is known by in its source: its key, or where in a document it was found. */
function keyOf(record: ObservationRecord): string {
  const name = observationName(record);
  return name.startsWith(`${record.source}:`) ? name.slice(record.source.length + 1) : name;
}

export function Observations({ route, types }: { route: Route; types: string[] }) {
  const { type, source, key, document, unassigned, q } = route.query;
  const pair = usePair();
  const pages = usePages(async (client, after) => {
    const page = await client.observations({
      entity_type: type,
      source,
      key,
      document,
      q,
      unassigned: unassigned === "true" ? true : undefined,
      after: after as string | undefined,
    });
    return { items: page.observations, next: page.next };
  });
  const labels = useLabels(
    pages.loaded.status === "ready" ? pages.loaded.data.map((observation) => observation.identity) : [],
  );
  return (
    <Page
      title="Records"
      about={
        document ? (
          <>
            The mentions an extractor found in document <Ref kind="document" id={document} />.
          </>
        ) : (
          "The observations the store holds: rows as they were ingested and mentions an extractor found, with the identity each belongs to now."
        )
      }
    >
      <Filters
        path={["records"]}
        query={route.query}
        search={{ name: "q", label: "Find", placeholder: "Words in a value, the source or the key" }}
        selects={[{ name: "type", label: "Entity type", choices: types.map((value) => ({ value })) }]}
        texts={[
          { name: "source", label: "Source" },
          { name: "key", label: "Key" },
        ]}
        flags={[{ name: "unassigned", label: "Without an identity" }]}
      />
      <PagedTable
        pages={pages}
        label="Records"
        columns={[
          { title: "", cell: (observation) => pair.tick(observation.id) },
          {
            title: "Record",
            cell: (observation) => (
              <span {...stylex.props(styles.name)}>
                <Ref kind="observation" id={observation.id} label={observation.label ?? observationName(observation)} />
                {observation.superseded_by ? <Word value="replaced" label="earlier version" /> : null}
              </span>
            ),
            wrap: true,
          },
          { title: "Source", cell: (observation) => <Source name={observation.source} /> },
          { title: "Key", cell: (observation) => <Code>{keyOf(observation)}</Code> },
          { title: "Entity type", cell: (observation) => observation.entity_type },
          {
            title: "Identity",
            cell: (observation) =>
              observation.identity ? (
                <Identity id={observation.identity} labels={labels} />
              ) : (
                <Word value="unassigned" label="none" />
              ),
          },
          { title: "ID", cell: (observation) => <Ref kind="observation" id={observation.id} /> },
        ]}
        rowKey={(observation) => observation.id}
        empty={q ? `No record holds “${q}”.` : "No records."}
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
  const record = loaded.status === "ready" ? loaded.data.observation : null;
  return (
    <Page
      title={record ? (record.label ?? observationName(record)) : "Record"}
      kind={
        <>
          <RecordIcon /> Record{record ? ` · ${record.entity_type}` : ""}
        </>
      }
      about={<Id value={id} />}
    >
      <Load state={loaded}>{(loaded) => <ObservationScreen {...loaded} />}</Load>
    </Page>
  );
}

function ObservationScreen({ observation, review, document }: Loaded) {
  const mention = mentionOf(observation);
  const around = mention && document?.text ? excerpt(document.text, mention.start, mention.end) : null;
  const waits = review.filter((item) => item.status === "open");
  const labels = useLabels([observation.identity]);
  return (
    <>
      {observation.superseded_by ? (
        <Notice tone="waits">
          <span>
            A later version of this row replaced it: <Ref kind="observation" id={observation.superseded_by} />.
          </span>
        </Notice>
      ) : null}
      {waits.length > 0 ? (
        <Notice tone="waits">
          <span>
            It has no identity until someone decides{" "}
            {waits.map((item, index) => (
              <span key={item.id}>
                {index > 0 ? ", " : ""}
                <Ref kind="review" id={item.id} label={`review item ${item.id}`} />
              </span>
            ))}
            .
          </span>
        </Notice>
      ) : null}
      <Fields
        rows={[
          ["Entity type", observation.entity_type],
          ["Source", <Source name={observation.source} />],
          ["Key", observation.key ? <Code>{observation.key}</Code> : null],
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
              <Identity id={observation.identity} labels={labels} />
            ) : (
              <Word value="unassigned" label="none" />
            ),
          ],
          ["Replaces", observation.supersedes ? <Ref kind="observation" id={observation.supersedes} /> : null],
          ["Replaced by", observation.superseded_by ? <Ref kind="observation" id={observation.superseded_by} /> : null],
          [
            "Review",
            review.length > 0 ? (
              <Row>
                {review.map((item) => (
                  <Row key={item.id}>
                    <Ref kind="review" id={item.id} label={`item ${item.id}`} /> <Word value={item.kind} />{" "}
                    <Word value={item.status} />
                  </Row>
                ))}
              </Row>
            ) : null,
          ],
        ]}
      />
      {mention && document ? <Found mention={mention} document={document} excerpt={around} /> : null}
      <Section title="What it says" about="Its payload, as it was ingested.">
        <Json value={observation.payload} />
      </Section>
      {observation.evidence !== undefined && observation.evidence !== null ? (
        <Section title="Evidence" about="What the source gave with it, to say where it came from.">
          <Json value={observation.evidence} />
        </Section>
      ) : null}
      <Section title="Explain" about="Why it is with another record, or apart from it.">
        <ExplainWith observation={observation} />
      </Section>
      <Move observation={observation} />
    </>
  );
}

/** Puts the observation in another identity, in a new one, or in none. */
function Move({ observation }: { observation: ObservationRecord }) {
  const [done, setDone] = useState<ReactNode>(null);
  const [chosen, setWhere] = useState("identity");
  const from = observation.identity ?? null;
  // Once it is out of its identity, it cannot be taken out again.
  const where = chosen === "none" && from === null ? "identity" : chosen;
  return (
    <Section
      title={from ? "Move it" : "Place it"}
      about={
        from
          ? "When it belongs to another identity, to one of its own, or to none."
          : "It has no identity. A reviewer can give it one."
      }
    >
      <Done>{done}</Done>
      <div>
        <Act
          label={from ? "Move" : "Place"}
          title={`${from ? "Move" : "Place"} ${observation.label ?? observationName(observation)}`}
          about="The move is refused if the record is no longer where this screen shows it."
          who
          why="Reason"
          act={async (client, { actor, reason, entered }) => {
            const ops: Op[] = [];
            let to: string | null = null;
            if (where === "new") {
              to = (await client.mintIds({ identities: 1 })).identities[0]!;
              ops.push({ op: "create_identity", entity_type: observation.entity_type, identity: to });
            } else if (where === "identity") {
              const reference = String(entered.get("identity") ?? "").trim();
              if (reference === "") throw new Mistake("Name the identity it goes to.");
              const identity = await findIdentity(client, reference);
              if (identity === null) throw new Mistake(`No identity is named ${reference}.`);
              to = identity.id;
            }
            // Where it is now is what the reviewer saw: if that has changed
            // since, the engine rejects the move.
            ops.push({ op: "assign", observation: observation.id, from, to });
            const committed = await client.commit({ actor, reason, ops });
            return (
              <>
                {to === null ? "Taken out of its identity" : from ? "Moved" : "Placed"} at <At committed={committed} />.
              </>
            );
          }}
          done={setDone}
        >
          <Labelled label="Where to">
            <Select
              value={where}
              onChange={setWhere}
              choices={[
                { value: "identity", label: "an identity there is" },
                { value: "new", label: "a new identity of its own" },
                ...(from ? [{ value: "none", label: "no identity" }] : []),
              ]}
            />
          </Labelled>
          {where === "identity" ? (
            <Labelled label="That identity">
              <Input
                name="identity"
                placeholder="its ID, or a record of it as source:key"
                spellCheck={false}
                autoComplete="off"
              />
            </Labelled>
          ) : null}
        </Act>
      </div>
    </Section>
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
      <Text>
        In document{" "}
        <a href={href(["documents", document.id], { at: mention.start })} {...stylex.props(styles.link)}>
          {named(document.source, document.key)}
        </a>
        , characters {mention.start} to {mention.end}.
      </Text>
      {excerpt ? (
        <blockquote data-part="excerpt" {...stylex.props(styles.excerpt)}>
          {excerpt.cutBefore ? "…" : ""}
          {excerpt.before}
          <mark {...stylex.props(styles.mark)}>{excerpt.text}</mark>
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
    <>
      <form
        {...stylex.props(styles.form)}
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
              else navigate(href(["explain"], { a: observation.id, b: other.id }));
            },
            (error: unknown) => {
              setBusy(false);
              setProblem(message(error));
            },
          );
        }}
      >
        <Labelled label="The other record">
          <Input
            name="other"
            placeholder="an ID, or source:key"
            spellCheck={false}
            autoComplete="off"
            style={styles.box}
          />
        </Labelled>
        <Button type="submit" variant="outline" disabled={busy}>
          Explain
        </Button>
      </form>
      {problem ? (
        <Notice tone="bad" role="alert">
          {problem}
        </Notice>
      ) : null}
    </>
  );
}

const styles = stylex.create({
  name: {
    gap: spacing["2"],
    alignItems: "center",
    display: "inline-flex",
    flexWrap: "wrap",
    fontWeight: typography.fontWeightMedium,
  },
  link: {
    color: tones.link,
    textDecorationLine: { default: "none", ":hover": "underline" },
  },
  excerpt: {
    margin: 0,
    borderInlineStartColor: colors.border,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: 3,
    borderRadius: radius.sm,
    paddingBlock: spacing["2"],
    paddingInline: spacing["4"],
    backgroundColor: colors.card,
    maxWidth: "52rem",
    overflowWrap: "anywhere",
  },
  mark: {
    borderRadius: radius.sm,
    paddingInline: "0.125rem",
    backgroundColor: tones.waitsSurface,
    color: tones.waits,
    fontWeight: typography.fontWeightMedium,
  },
  form: {
    gap: spacing["3"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
  },
  box: {
    width: "24rem",
    maxWidth: "100%",
  },
});
