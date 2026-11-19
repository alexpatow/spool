import * as stylex from "@stylexjs/stylex";
import { useEffect, useMemo, useRef } from "react";

import type { DocumentEntry, DocumentSort, RecordEntry, SpoolClient } from "../api.ts";
import {
  Code,
  Fields,
  Filters,
  Id,
  Placed,
  LinkTo,
  Load,
  Page,
  PagedTable,
  Quiet,
  Ref,
  Section,
  Source,
  Table,
  usePair,
  useSorting,
  When,
  Word,
} from "../components.tsx";
import { FileTextIcon } from "../icons.tsx";
import { colors, radius, spacing, typography } from "../styles/shelf/tokens.stylex";
import { tones } from "../styles/tones.stylex";
import { codePoints, count, mentionOf, named, readBy, segments } from "../format.ts";
import type { Mention } from "../format.ts";
import { useLabels, useLoad, usePages } from "../hooks.ts";
import { href } from "../router.ts";
import type { Route } from "../router.ts";

export function Documents({ route }: { route: Route }) {
  const { source, key, latest } = route.query;
  const sorting = useSorting(route, { by: "key", order: "asc" });
  const pages = usePages(async (client, after) => {
    const page = await client.documents({
      source,
      key,
      latest: latest === "true" ? true : undefined,
      after: after as string | undefined,
      sort: sorting.by as DocumentSort,
      order: sorting.order,
    });
    return { items: page.documents, next: page.next };
  });
  return (
    <Page title="Documents">
      <Filters
        path={["documents"]}
        query={route.query}
        texts={[
          { name: "source", label: "Source" },
          { name: "key", label: "Key" },
        ]}
        flags={[{ name: "latest", label: "Latest versions only" }]}
      />
      <PagedTable
        pages={pages}
        sorting={sorting}
        columns={[
          {
            title: "Document",
            sort: "key",
            cell: (document) => (
              <span {...stylex.props(styles.name)}>
                <Ref kind="document" id={document.id} label={named(document.source, document.key)} />
                {document.superseded_by ? <Word value="replaced" label="superseded" /> : null}
              </span>
            ),
          },
          { title: "Source", cell: (document) => <Source name={document.source} />, sort: "source" },
          { title: "Characters", cell: (document) => document.length, number: true, sort: "length" },
          { title: "Loaded", cell: (document) => <When at={document.loaded_at} />, sort: "loaded", first: "desc" },
          { title: "Extracted by", cell: readBy, wrap: true },
          { title: "ID", cell: (document) => <Ref kind="document" id={document.id} />, sort: "id" },
        ]}
        rowKey={(document) => document.id}
        empty="No documents."
      />
    </Page>
  );
}

/** The most mentions of one document the studio reads to mark them in its text. */
const MOST_MENTIONS = 5_000;

interface Found {
  record: RecordEntry;
  mention: Mention;
}

interface Loaded {
  document: DocumentEntry;
  found: Found[];
  /** Whether the document has more mentions than were read. */
  cut: boolean;
}

async function mentions(client: SpoolClient, document: string): Promise<{ found: Found[]; cut: boolean }> {
  const found: Found[] = [];
  let after: string | undefined;
  for (;;) {
    const page = await client.records({ document, after, limit: 1000 });
    for (const record of page.records) {
      const mention = mentionOf(record);
      if (mention !== null) found.push({ record, mention });
    }
    if (!page.next) return { found, cut: false };
    if (found.length >= MOST_MENTIONS) return { found, cut: true };
    after = page.next;
  }
}

export function Document({ id, route }: { id: string; route: Route }) {
  const loaded = useLoad(async (client): Promise<Loaded> => {
    const [document, read] = await Promise.all([client.document(id), mentions(client, id)]);
    read.found.sort((a, b) => a.mention.start - b.mention.start || a.mention.end - b.mention.end);
    return { document, ...read };
  });
  const at = route.query.at === undefined ? null : Number(route.query.at);
  return (
    <Page
      title={loaded.status === "ready" ? named(loaded.data.document.source, loaded.data.document.key) : "Document"}
      kind={
        <>
          <FileTextIcon /> Document
        </>
      }
      about={<Id value={id} />}
    >
      <Load state={loaded}>{(loaded) => <DocumentScreen {...loaded} at={at} />}</Load>
    </Page>
  );
}

function DocumentScreen({ document, found, cut, at }: Loaded & { at: number | null }) {
  const labels = useLabels(found.map(({ record }) => record.identity));
  const pair = usePair();
  // Taken apart once: the marked text and every label of the table read it.
  const points = useMemo(() => codePoints(document.text ?? ""), [document.text]);
  return (
    <>
      <Fields
        rows={[
          ["Source", <Source name={document.source} />],
          ["Key", <Code>{document.key}</Code>],
          ["URI", document.uri],
          ["Characters", document.length],
          [
            "Loaded",
            <>
              <When at={document.loaded_at} /> by{" "}
              <Ref kind="run" id={document.load_run} label={`run ${document.load_run}`} />
            </>,
          ],
          ["Supersedes", document.supersedes ? <Ref kind="document" id={document.supersedes} /> : null],
          ["Superseded by", document.superseded_by ? <Ref kind="document" id={document.superseded_by} /> : null],
          [
            "Extracted by",
            document.extractions.length === 0 && !document.reading ? (
              "not read yet"
            ) : (
              <>
                {document.extractions.map((extraction, index) => (
                  <span key={extraction.run}>
                    {index > 0 ? ", " : ""}
                    <Code>{extraction.extractor}</Code>
                    {extraction.failure ? " failed in " : " in "}
                    <Ref kind="run" id={extraction.run} label={`run ${extraction.run}`} />
                    {extraction.failure ? `: ${extraction.failure}` : ` (${count(extraction.mentions, "mention")})`}
                  </span>
                ))}
                {document.reading ? (
                  <span>
                    {document.extractions.length > 0 ? ", " : ""}being read by <Code>{document.reading.extractor}</Code>{" "}
                    in <Ref kind="run" id={document.reading.run} label={`run ${document.reading.run}`} />
                  </span>
                ) : null}
              </>
            ),
          ],
        ]}
      />
      <Section title="Text">
        <Marked points={points} found={found} at={at} />
        {cut ? (
          <Quiet>
            First {found.length} mentions marked.{" "}
            <LinkTo path={["records"]} query={{ document: document.id }}>
              All mentions
            </LinkTo>
          </Quiet>
        ) : null}
      </Section>
      <Section title="Mentions" count={found.length} actions={found.length > 1 ? pair.explain : null}>
        <Table
          columns={[
            { title: "", cell: ({ record }) => pair.tick(record.id) },
            {
              title: "Mention",
              value: ({ mention }) => points.slice(mention.start, mention.end).join(""),
              cell: ({ record, mention }) => (
                <a href={href(["records", record.id])} {...stylex.props(styles.link)}>
                  {points.slice(mention.start, mention.end).join("")}
                </a>
              ),
            },
            {
              title: "Characters",
              value: ({ mention }) => mention.start,
              first: "asc",
              cell: ({ mention }) => `${mention.start} to ${mention.end}`,
            },
            {
              title: "Entity type",
              value: ({ record }) => record.entity_type,
              cell: ({ record }) => record.entity_type,
            },
            {
              title: "Identity",
              value: ({ record }) => (record.identity ? (labels.get(record.identity) ?? record.identity) : null),
              cell: ({ record }) =>
                <Placed record={record} labels={labels} />,
            },
          ]}
          rows={found}
          rowKey={({ record }) => record.id}
          initial={{ by: "Characters", order: "asc" }}
          empty="No mentions."
        />
      </Section>
    </>
  );
}

/** The text with each mention marked. A marked stretch leads to its record. */
function Marked({ points, found, at }: { points: string[]; found: Found[]; at: number | null }) {
  const target = useRef<HTMLAnchorElement | null>(null);
  useEffect(() => {
    target.current?.scrollIntoView?.({ block: "center" });
  }, [at]);
  // Ticking a mention shows the screen again, and the text is cut as it was.
  const pieces = useMemo(
    () =>
      segments(
        points,
        found.map(({ record, mention }) => ({
          start: mention.start,
          end: mention.end,
          value: { record, mention },
        })),
      ),
    [points, found],
  );
  return (
    <pre data-part="document" {...stylex.props(styles.document)}>
      {pieces.map((piece) => {
        const first = piece.covered[0];
        if (first === undefined) return <span key={piece.start}>{piece.text}</span>;
        const here = at !== null && piece.covered.some(({ mention }) => mention.start === at);
        return (
          <a
            key={piece.start}
            ref={here ? target : undefined}
            data-part="mention"
            data-here={here ? "" : undefined}
            {...stylex.props(styles.mention, here && styles.here)}
            href={href(["records", first.record.id])}
            title={piece.covered.map(({ record }) => `${record.entity_type} ${record.id}`).join("\n")}
          >
            {piece.text}
          </a>
        );
      })}
    </pre>
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
    color: colors.foreground,
    fontWeight: typography.fontWeightMedium,
    textDecorationColor: { default: colors.ring, ":hover": colors.foreground },
    textDecorationLine: "underline",
    textDecorationThickness: "1px",
    textUnderlineOffset: "0.2em",
  },
  document: {
    margin: 0,
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    padding: spacing["4"],
    backgroundColor: colors.card,
    fontFamily: typography.fontFamily,
    fontSize: typography.fontSizeSm,
    lineHeight: "1.6",
    maxHeight: "32rem",
    overflowWrap: "anywhere",
    overflowY: "auto",
    whiteSpace: "pre-wrap",
  },
  mention: {
    borderRadius: radius.sm,
    paddingBlock: "0.0625rem",
    backgroundColor: { default: tones.waitsSurface, ":hover": tones.waitsMark },
    color: { default: tones.waits, ":hover": colors.background },
    textDecorationLine: "none",
  },
  here: {
    outlineColor: tones.waitsMark,
    outlineOffset: 1,
    outlineStyle: "solid",
    outlineWidth: 2,
  },
});
