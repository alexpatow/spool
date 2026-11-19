import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done, Mistake } from "../act.tsx";
import type { DocumentEntry, RecordEntry, RecordSort, Op, ReviewEntry } from "../api.ts";
import {
  Code,
  Fields,
  Filters,
  Id,
  Placed,
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
  useSorting,
  When,
  Word,
} from "../components.tsx";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { findIdentity, findRecord, found } from "../find.ts";
import { excerpt, mentionOf, named, recordName } from "../format.ts";
import type { Excerpt, Mention } from "../format.ts";
import { useLabels, useLoad, usePages, useStudio } from "../hooks.ts";
import { RecordIcon } from "../icons.tsx";
import { href, navigate } from "../router.ts";
import type { Route } from "../router.ts";
import { colors, radius, spacing, typography } from "../styles/shelf/tokens.stylex";
import { tones } from "../styles/tones.stylex";

/** What a record is known by in its source: its key, or where in a document it was found. */
function keyOf(record: RecordEntry): string {
  const name = recordName(record);
  return name.startsWith(`${record.source}:`) ? name.slice(record.source.length + 1) : name;
}

export function RecordsPage({ route, types }: { route: Route; types: string[] }) {
  const { type, source, key, document, unassigned, q } = route.query;
  const pair = usePair();
  const sorting = useSorting(route, { by: "source", order: "asc" });
  const pages = usePages(async (client, after) => {
    const page = await client.records({
      sort: sorting.by as RecordSort,
      order: sorting.order,
      entity_type: type,
      source,
      key,
      document,
      q,
      unassigned: unassigned === "true" ? true : undefined,
      after: after as string | undefined,
    });
    return { items: page.records, next: page.next };
  });
  const labels = useLabels(pages.loaded.status === "ready" ? pages.loaded.data.map((record) => record.identity) : []);
  return (
    <Page
      title="Records"
      about={
        document ? (
          <>
            Mentions in document <Ref kind="document" id={document} />
          </>
        ) : undefined
      }
    >
      <Filters
        path={["records"]}
        query={route.query}
        search={{ name: "q", label: "Search", placeholder: "Value, source or key" }}
        selects={[{ name: "type", label: "Entity type", choices: types.map((value) => ({ value })) }]}
        texts={[
          { name: "source", label: "Source" },
          { name: "key", label: "Key" },
        ]}
        flags={[{ name: "unassigned", label: "Unassigned" }]}
      />
      <PagedTable
        pages={pages}
        label="Records"
        sorting={sorting}
        columns={[
          { title: "", cell: (record) => pair.tick(record.id) },
          {
            title: "Record",
            sort: "label",
            cell: (record) => (
              <span {...stylex.props(styles.name)}>
                <Ref kind="record" id={record.id} label={record.label ?? recordName(record)} />
                {record.superseded_by ? <Word value="replaced" label="superseded" /> : null}
              </span>
            ),
            wrap: true,
          },
          { title: "Source", cell: (record) => <Source name={record.source} />, sort: "source" },
          { title: "Key", cell: (record) => <Code>{keyOf(record)}</Code>, sort: "key" },
          { title: "Entity type", cell: (record) => record.entity_type, sort: "entity_type" },
          {
            title: "Identity",
            sort: "identity",
            cell: (record) =>
              <Placed record={record} labels={labels} />,
          },
          { title: "ID", cell: (record) => <Ref kind="record" id={record.id} />, sort: "id" },
        ]}
        rowKey={(record) => record.id}
        empty={q ? `No records match “${q}”.` : "No records."}
      />
      {pair.explain}
    </Page>
  );
}

interface Loaded {
  record: RecordEntry;
  review: ReviewEntry[];
  /** The document a mention was found in, if the store still has it. */
  document: DocumentEntry | null;
}

export function RecordPage({ id }: { id: string }) {
  const loaded = useLoad(async (client): Promise<Loaded> => {
    const [record, review] = await Promise.all([client.record(id), client.reviewItems({ record: id })]);
    const mention = mentionOf(record);
    const document = mention ? await found(client.document(mention.document)) : null;
    return { record, review, document };
  });
  const record = loaded.status === "ready" ? loaded.data.record : null;
  return (
    <Page
      title={record ? (record.label ?? recordName(record)) : "Record"}
      kind={
        <>
          <RecordIcon /> Record{record ? ` · ${record.entity_type}` : ""}
        </>
      }
      about={<Id value={id} />}
    >
      <Load state={loaded}>{(loaded) => <RecordScreen {...loaded} />}</Load>
    </Page>
  );
}

function RecordScreen({ record, review, document }: Loaded) {
  const mention = mentionOf(record);
  const around = mention && document?.text ? excerpt(document.text, mention.start, mention.end) : null;
  const open = review.filter((item) => item.status === "open");
  // A `possible_split` is about the identity the record is in, not about a record that waits for one.
  const waits = open.filter((item) => item.kind !== "possible_split");
  const asks = open.filter((item) => item.kind === "possible_split");
  const labels = useLabels([record.identity]);
  return (
    <>
      {record.superseded_by ? (
        <Notice tone="waits">
          <span>
            Superseded by <Ref kind="record" id={record.superseded_by} />.
          </span>
        </Notice>
      ) : null}
      {record.retracted != null ? (
        <Notice tone="waits">
          <span>
            Taken back by its source in{" "}
            <Ref kind="run" id={record.retracted} label={`run ${record.retracted}`} />. It has no identity, and no run
            examines it, until the source sends the row again.
          </span>
        </Notice>
      ) : null}
      {waits.length > 0 ? (
        <Notice tone="waits">
          <span>
            Unassigned, pending{" "}
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
      {asks.length > 0 ? (
        <Notice tone="waits">
          <span>
            Its identity lost a record that its source took back.{" "}
            {asks.map((item, index) => (
              <span key={item.id}>
                {index > 0 ? ", " : ""}
                <Ref kind="review" id={item.id} label={`Review item ${item.id}`} />
              </span>
            ))}{" "}
            asks whether the records that are left are still one thing.
          </span>
        </Notice>
      ) : null}
      <Fields
        rows={[
          ["Entity type", record.entity_type],
          ["Source", <Source name={record.source} />],
          ["Key", record.key ? <Code>{record.key}</Code> : null],
          [
            "Loaded",
            <>
              <When at={record.loaded_at} /> by{" "}
              <Ref kind="run" id={record.load_run} label={`run ${record.load_run}`} />
            </>,
          ],
          [
            "Identity",
            <Placed record={record} labels={labels} />,
          ],
          ["Supersedes", record.supersedes ? <Ref kind="record" id={record.supersedes} /> : null],
          ["Superseded by", record.superseded_by ? <Ref kind="record" id={record.superseded_by} /> : null],
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
      <Section title="Payload">
        <Json value={record.payload} />
      </Section>
      {record.evidence !== undefined && record.evidence !== null ? (
        <Section title="Evidence">
          <Json value={record.evidence} />
        </Section>
      ) : null}
      <Section title="Explain">
        <ExplainWith record={record} />
      </Section>
      {record.retracted == null ? <Move record={record} /> : null}
    </>
  );
}

/** Puts the record in another identity, in a new one, or in none. */
function Move({ record }: { record: RecordEntry }) {
  const [done, setDone] = useState<ReactNode>(null);
  const [chosen, setWhere] = useState("identity");
  const from = record.identity ?? null;
  // Once it is out of its identity, it cannot be taken out again.
  const where = chosen === "none" && from === null ? "identity" : chosen;
  return (
    <Section title={from ? "Move" : "Assign"}>
      <Done>{done}</Done>
      <div>
        <Act
          label={from ? "Move" : "Assign"}
          title={`${from ? "Move" : "Assign"} ${record.label ?? recordName(record)}`}
          why="Reason"
          act={async (client, { reason, entered }) => {
            const ops: Op[] = [];
            let to: string | null = null;
            if (where === "new") {
              to = (await client.mintIds({ identities: 1 })).identities[0]!;
              ops.push({ op: "create_identity", entity_type: record.entity_type, identity: to });
            } else if (where === "identity") {
              const reference = String(entered.get("identity") ?? "").trim();
              if (reference === "") throw new Mistake("Enter an identity.");
              const identity = await findIdentity(client, reference);
              if (identity === null) throw new Mistake(`No identity found for ${reference}.`);
              to = identity.id;
            }
            // Where it is now is what the reviewer saw: if that has changed
            // since, the engine rejects the move.
            ops.push({ op: "assign", record: record.id, from, to });
            const committed = await client.commit({ reason, ops });
            return (
              <>
                {to === null ? "Unassigned" : from ? "Moved" : "Assigned"} at <At committed={committed} />.
              </>
            );
          }}
          done={setDone}
        >
          <Labelled label="To">
            <Select
              value={where}
              onChange={setWhere}
              choices={[
                { value: "identity", label: "Existing identity" },
                { value: "new", label: "New identity" },
                ...(from ? [{ value: "none", label: "No identity (unassign)" }] : []),
              ]}
            />
          </Labelled>
          {where === "identity" ? (
            <Labelled label="Identity">
              <Input
                name="identity"
                placeholder="ID, or source:key of one of its records"
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

function Found({ mention, document, excerpt }: { mention: Mention; document: DocumentEntry; excerpt: Excerpt | null }) {
  return (
    <Section title="Mention">
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

/** Asks for the other record of a pair, and goes to the explanation of the two. */
function ExplainWith({ record }: { record: RecordEntry }) {
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
          findRecord(client, reference).then(
            (other) => {
              setBusy(false);
              if (other === null) setProblem(`No record found for ${reference}.`);
              else navigate(href(["explain"], { a: record.id, b: other.id }));
            },
            (error: unknown) => {
              setBusy(false);
              setProblem(message(error));
            },
          );
        }}
      >
        <Labelled label="Compare with">
          <Input name="other" placeholder="ID or source:key" spellCheck={false} autoComplete="off" style={styles.box} />
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
