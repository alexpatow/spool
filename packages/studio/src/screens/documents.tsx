import { useEffect, useRef } from "react";

import type { DocumentRecord, ObservationRecord, SpoolClient } from "../api.ts";
import {
  Fields,
  Filters,
  Id,
  LinkTo,
  Load,
  Page,
  PagedTable,
  Ref,
  Section,
  Table,
  When,
  Word,
  usePair,
} from "../components.tsx";
import { count, mentionOf, named, segments } from "../format.ts";
import type { Mention } from "../format.ts";
import { useLoad, usePages } from "../hooks.ts";
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
            cell: (document) => <Ref kind="document" id={document.id} label={named(document.source, document.key)} />,
          },
          { title: "ID", cell: (document) => <Ref kind="document" id={document.id} /> },
          { title: "Characters", cell: (document) => document.length, number: true },
          { title: "Read by", cell: readBy },
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
    <Page title="Document" about={<Id value={id} />}>
      <Load state={loaded}>{(loaded) => <DocumentScreen {...loaded} at={at} />}</Load>
    </Page>
  );
}

function DocumentScreen({ document, found, cut, at }: Loaded & { at: number | null }) {
  const pair = usePair();
  return (
    <>
      <Fields
        rows={[
          ["Source", document.source],
          ["Key", <code>{document.key}</code>],
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
                    <code>{extraction.extractor}</code> in{" "}
                    <Ref kind="run" id={extraction.run} label={`run ${extraction.run}`} /> (
                    {count(extraction.mentions, "mention")})
                  </span>
                )),
          ],
        ]}
      />
      <Section title="Text">
        <Text text={document.text ?? ""} found={found} at={at} />
        {cut ? (
          <p className="quiet">
            Only the first {found.length} mentions are marked.{" "}
            <LinkTo path={["observations"]} query={{ document: document.id }}>
              List them all
            </LinkTo>
          </p>
        ) : null}
      </Section>
      <Section title={`Mentions (${found.length}${cut ? " and more" : ""})`}>
        <Table
          columns={[
            { title: "", cell: ({ observation }) => pair.tick(observation.id) },
            {
              title: "Mention",
              cell: ({ observation, mention }) => (
                <a href={href(["observations", observation.id])}>{mentionText(document, mention)}</a>
              ),
            },
            { title: "Characters", cell: ({ mention }) => `${mention.start} to ${mention.end}` },
            { title: "Entity type", cell: ({ observation }) => observation.entity_type },
            {
              title: "Identity",
              cell: ({ observation }) =>
                observation.identity ? (
                  <Ref kind="identity" id={observation.identity} />
                ) : (
                  <Word value="unassigned" label="none" />
                ),
            },
          ]}
          rows={found}
          rowKey={({ observation }) => observation.id}
          empty="No extractor found a mention in this version of the document."
        />
        {found.length > 1 ? pair.explain : null}
      </Section>
    </>
  );
}

function mentionText(document: DocumentRecord, mention: Mention): string {
  return Array.from(document.text ?? "")
    .slice(mention.start, mention.end)
    .join("");
}

/** The text with each mention marked. A marked stretch leads to its observation. */
function Text({ text, found, at }: { text: string; found: Found[]; at: number | null }) {
  const target = useRef<HTMLAnchorElement | null>(null);
  useEffect(() => {
    target.current?.scrollIntoView?.({ block: "center" });
  }, [at]);
  const pieces = segments(
    text,
    found.map(({ observation, mention }) => ({
      start: mention.start,
      end: mention.end,
      value: { observation, mention },
    })),
  );
  return (
    <pre className="document">
      {pieces.map((piece) => {
        const first = piece.covered[0];
        if (first === undefined) return <span key={piece.start}>{piece.text}</span>;
        const here = at !== null && piece.covered.some(({ mention }) => mention.start === at);
        return (
          <a
            key={piece.start}
            ref={here ? target : undefined}
            className={`mention ${here ? "here" : ""}`}
            href={href(["observations", first.observation.id])}
            title={piece.covered.map(({ observation }) => `${observation.entity_type} ${observation.id}`).join("\n")}
          >
            {piece.text}
          </a>
        );
      })}
    </pre>
  );
}
