import * as stylex from "@stylexjs/stylex";
import { useEffect, useMemo, useRef } from "react";

import type { DocumentRecord, ObservationRecord, SpoolClient } from "../api.ts";
import {
  Code,
  Fields,
  Filters,
  Id,
  Identity,
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
  When,
  Word,
} from "../components.tsx";
import { FileTextIcon } from "../icons.tsx";
import { colors, radius, spacing, typography } from "../styles/shelf/tokens.stylex";
import { tones } from "../styles/tones.stylex";
import { codePoints, count, mentionOf, named, segments } from "../format.ts";
import type { Mention } from "../format.ts";
import { useLabels, useLoad, usePages } from "../hooks.ts";
import { href } from "../router.ts";
import type { Route } from "../router.ts";

function readBy(document: DocumentRecord): string {
  if (document.extractions.length === 0) return "no extractor";
  return document.extractions
    .map((extraction) => `${extraction.extractor} in run ${extraction.run} (${count(extraction.mentions, "mention")})`)
    .join(", ");
}

export function Documents({ route }: { route: Route }) {
  const { source, key, latest } = route.query;
  const pages = usePages(async (client, after) => {
    const page = await client.documents({
      source,
      key,
      latest: latest === "true" ? true : undefined,
      after: after as string | undefined,
    });
    return { items: page.documents, next: page.next };
  });
  return (
    <Page title="Documents" about="The texts extractors read. Each version of a document is listed.">
      <Filters
        path={["documents"]}
        query={route.query}
        texts={[
          { name: "source", label: "Source" },
          { name: "key", label: "Key" },
        ]}
        flags={[{ name: "latest", label: "Only the latest version of each" }]}
      />
      <PagedTable
        pages={pages}
        columns={[
          {
            title: "Document",
            cell: (document) => (
              <span {...stylex.props(styles.name)}>
                <Ref kind="document" id={document.id} label={named(document.source, document.key)} />
                {document.superseded_by ? <Word value="replaced" label="earlier version" /> : null}
              </span>
            ),
          },
          { title: "Source", cell: (document) => <Source name={document.source} /> },
          { title: "Characters", cell: (document) => document.length, number: true },
          { title: "Read by", cell: readBy, wrap: true },
          { title: "ID", cell: (document) => <Ref kind="document" id={document.id} /> },
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
  observation: ObservationRecord;
  mention: Mention;
}

interface Loaded {
  document: DocumentRecord;
  found: Found[];
  /** Whether the document has more mentions than were read. */
  cut: boolean;
}

async function mentions(client: SpoolClient, document: string): Promise<{ found: Found[]; cut: boolean }> {
  const found: Found[] = [];
  let after: string | undefined;
  for (;;) {
    const page = await client.observations({ document, after, limit: 1000 });
    for (const observation of page.observations) {
      const mention = mentionOf(observation);
      if (mention !== null) found.push({ observation, mention });
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
  const labels = useLabels(found.map(({ observation }) => observation.identity));
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
            "Ingested",
            <>
              <When at={document.ingested_at} /> by{" "}
              <Ref kind="run" id={document.ingest_run} label={`run ${document.ingest_run}`} />
            </>,
          ],
          ["Replaces", document.supersedes ? <Ref kind="document" id={document.supersedes} /> : null],
          ["Replaced by", document.superseded_by ? <Ref kind="document" id={document.superseded_by} /> : null],
          [
            "Read by",
            document.extractions.length === 0
              ? "no extractor"
              : document.extractions.map((extraction, index) => (
                  <span key={extraction.run}>
                    {index > 0 ? ", " : ""}
                    <Code>{extraction.extractor}</Code> in{" "}
                    <Ref kind="run" id={extraction.run} label={`run ${extraction.run}`} /> (
                    {count(extraction.mentions, "mention")})
                  </span>
                )),
          ],
        ]}
      />
      <Section title="Text" about="Each mention an extractor found is marked, and leads to its record.">
        <Marked points={points} found={found} at={at} />
        {cut ? (
          <Quiet>
            Only the first {found.length} mentions are marked.{" "}
            <LinkTo path={["records"]} query={{ document: document.id }}>
              List them all
            </LinkTo>
          </Quiet>
        ) : null}
      </Section>
      <Section
        title={cut ? "Mentions, the first of them" : "Mentions"}
        count={found.length}
        actions={found.length > 1 ? pair.explain : null}
      >
        <Table
          columns={[
            { title: "", cell: ({ observation }) => pair.tick(observation.id) },
            {
              title: "Mention",
              cell: ({ observation, mention }) => (
                <a href={href(["records", observation.id])} {...stylex.props(styles.link)}>
                  {points.slice(mention.start, mention.end).join("")}
                </a>
              ),
            },
            { title: "Characters", cell: ({ mention }) => `${mention.start} to ${mention.end}` },
            { title: "Entity type", cell: ({ observation }) => observation.entity_type },
            {
              title: "Identity",
              cell: ({ observation }) =>
                observation.identity ? (
                  <Identity id={observation.identity} labels={labels} />
                ) : (
                  <Word value="unassigned" label="none" />
                ),
            },
          ]}
          rows={found}
          rowKey={({ observation }) => observation.id}
          empty="No extractor found a mention in this version of the document."
        />
      </Section>
    </>
  );
}

/** The text with each mention marked. A marked stretch leads to its observation. */
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
        found.map(({ observation, mention }) => ({
          start: mention.start,
          end: mention.end,
          value: { observation, mention },
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
            href={href(["records", first.observation.id])}
            title={piece.covered.map(({ observation }) => `${observation.entity_type} ${observation.id}`).join("\n")}
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
    color: tones.link,
    fontWeight: typography.fontWeightMedium,
    textDecorationLine: { default: "none", ":hover": "underline" },
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
