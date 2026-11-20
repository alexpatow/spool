import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import type { Info, RunEntry, TypeInfo } from "../api.ts";
import {
  Code,
  Fields,
  LinkTo,
  Load,
  Notice,
  Onward,
  Page,
  Quiet,
  Ref,
  ScreenLink,
  Section,
  Source,
  Table,
  When,
  Word,
} from "../components.tsx";
import type { Leads } from "../components.tsx";
import { count } from "../format.ts";
import { keys, useRead } from "../queries.ts";
import { colors, display, linen, radius, spacing, typography } from "spool-ui/tokens.stylex";
import { tones } from "../styles/tones.stylex";
import { useCalled, Who } from "../who.tsx";

/** How many of the latest runs the overview lists. */
const RUNS = 5;

/** What a run was about, as a list of runs says it. */
function details(run: RunEntry): string {
  return [run.entity_type, run.strategy?.name, run.extractor?.name].filter((part) => part).join(", ");
}

export function Overview() {
  const loaded = useRead(keys.info.overview, async (client) => {
    const [info, runs] = await Promise.all([client.info(), client.runs({ limit: RUNS })]);
    return { info, runs };
  });
  return (
    <Page title="Overview">
      <Load state={loaded}>{({ info, runs }) => <Store info={info} runs={runs} />}</Load>
    </Page>
  );
}

function Store({ info, runs }: { info: Info; runs: RunEntry[] }) {
  const { store } = info;
  const called = useCalled();
  return (
    <>
      <Waiting types={store.entity_types} />
      {store.entity_types.length === 0 ? <Quiet>No entity types declared.</Quiet> : null}
      {store.entity_types.map((type) => (
        <Section key={type.name} title={`Entity type ${type.name}`}>
          <div {...stylex.props(styles.stats)}>
            <Stat
              number={type.identities}
              label={type.identities === 1 ? "identity" : "identities"}
              leads={{ to: "/identities", search: { type: type.name } }}
            />
            <Stat
              number={type.records}
              label={type.records === 1 ? "record" : "records"}
              leads={{ to: "/records", search: { type: type.name } }}
            />
            <Stat
              number={type.unassigned}
              label="unassigned"
              leads={{ to: "/records", search: { type: type.name, unassigned: true } }}
              waits={type.unassigned > 0}
            />
            <Stat
              number={type.open_review_items}
              label={type.open_review_items === 1 ? "open review item" : "open review items"}
              leads={{ to: "/review", search: { type: type.name } }}
              waits={type.open_review_items > 0}
            />
          </div>
          <Table
            label={`${type.name} sources`}
            columns={[
              {
                title: "Source",
                value: (source) => source.source,
                cell: (source) => (
                  <ScreenLink
                    to="/records"
                    search={{ type: type.name, source: source.source }}
                    {...stylex.props(styles.plain)}
                  >
                    <Source name={source.source} />
                  </ScreenLink>
                ),
              },
              { title: "Records", value: (source) => source.records, cell: (source) => source.records, number: true },
              {
                title: "Unassigned",
                value: (source) => source.unassigned,
                cell: (source) =>
                  source.unassigned > 0 ? (
                    <LinkTo to="/records" search={{ type: type.name, source: source.source, unassigned: true }}>
                      {source.unassigned}
                    </LinkTo>
                  ) : (
                    0
                  ),
                number: true,
              },
              {
                title: "Last loaded",
                value: (source) => source.last_loaded_at,
                first: "desc",
                cell: (source) => <When at={source.last_loaded_at} />,
              },
            ]}
            rows={type.sources}
            rowKey={(source) => source.source}
            empty="No records yet."
          />
          <Fields
            rows={[
              [
                "Features",
                type.features.length > 0 ? (
                  <span {...stylex.props(styles.list)}>
                    {type.features.map((column) => (
                      <span key={column.name}>
                        <Code>{column.name}</Code>{" "}
                        <span {...stylex.props(styles.aside)}>{column.data_type.toLowerCase()}</span>
                      </span>
                    ))}
                  </span>
                ) : null,
              ],
              [
                "Strategies",
                <span {...stylex.props(styles.list)}>
                  <Code>rules</Code>
                  {(type.strategies ?? []).map((strategy) => (
                    <span key={strategy.name}>
                      <Code>{strategy.name}</Code> <span {...stylex.props(styles.aside)}>{strategy.runtime}</span>
                    </span>
                  ))}
                </span>,
              ],
              ["Declaration", `version ${type.declaration_version}`],
            ]}
          />
        </Section>
      ))}
      <Section title="Latest runs" actions={<Onward to="/runs">All runs</Onward>}>
        <Table
          label="Latest runs"
          columns={[
            {
              title: "Run",
              value: (run) => run.run,
              first: "desc",
              cell: (run) => <Ref kind="run" id={run.run} label={`Run ${run.run}`} />,
            },
            { title: "Kind", value: (run) => run.kind, cell: (run) => run.kind },
            { title: "Status", value: (run) => run.status, cell: (run) => <Word value={run.status} /> },
            {
              title: "Details",
              value: details,
              cell: details,
            },
            {
              title: "Started by",
              value: (run) => called(run.started_by),
              cell: (run) => <Who actor={run.started_by} />,
            },
            {
              title: "Started",
              value: (run) => run.started_at,
              first: "desc",
              cell: (run) => <When at={run.started_at} />,
            },
          ]}
          rows={runs}
          rowKey={(run) => run.run}
          empty="No runs yet."
        />
      </Section>
      {(store.extractors ?? []).length > 0 ? (
        <Section title="Extractors">
          <Table
            columns={[
              {
                title: "Extractor",
                value: (extractor) => extractor.name,
                cell: (extractor) => <Code>{extractor.name}</Code>,
              },
              { title: "Runtime", value: (extractor) => extractor.runtime, cell: (extractor) => extractor.runtime },
              {
                title: "File",
                value: (extractor) => extractor.file,
                cell: (extractor) => <Code>{extractor.file}</Code>,
              },
            ]}
            rows={store.extractors ?? []}
            rowKey={(extractor) => extractor.name}
            empty="No extractors."
          />
        </Section>
      ) : null}
      <Section title="Store">
        <Fields
          rows={[
            ["API", `${info.api}, engine ${store.engine_version}, store format ${store.format_version}`],
            ["Manifest", store.manifest_hash ? <Code>{store.manifest_hash}</Code> : "none applied"],
            ["Head", <LinkTo to="/log">log position {store.head}</LinkTo>],
            ["Documents", <LinkTo to="/documents">{store.documents ?? 0}</LinkTo>],
          ]}
        />
      </Section>
    </>
  );
}

/** What waits for a person, first of all: the reason most readers open the studio. */
function Waiting({ types }: { types: TypeInfo[] }) {
  const lines: ReactNode[] = [];
  for (const type of types) {
    if (type.open_review_items > 0) {
      lines.push(
        <span key={`${type.name}-review`}>
          {count(type.open_review_items, "open review item")} ({type.name}){" "}
          <Onward to="/review" search={{ type: types.length > 1 ? type.name : undefined }}>
            Review
          </Onward>
        </span>,
      );
    }
    if (type.open_draft) {
      lines.push(
        <span key={`${type.name}-draft`}>
          1 draft to approve ({type.name}) <Onward to="/drafts/$id" params={{ id: type.open_draft }}>Open</Onward>
        </span>,
      );
    }
  }
  if (lines.length === 0) {
    return types.length > 0 ? <Notice tone="good">Nothing to review.</Notice> : null;
  }
  return <Notice tone="waits">{lines}</Notice>;
}

function Stat({
  number,
  label,
  leads,
  waits = false,
}: {
  number: number;
  label: string;
  /** The list the number counts. */
  leads: Leads;
  waits?: boolean;
}) {
  return (
    <ScreenLink {...leads} data-part="stat" data-waits={waits ? "" : undefined} {...stylex.props(styles.stat)}>
      <strong {...stylex.props(styles.number, waits && styles.waits)}>{number.toLocaleString("en-GB")}</strong>
      <span {...stylex.props(styles.aside)}>{label}</span>
    </ScreenLink>
  );
}

const styles = stylex.create({
  stats: {
    gap: spacing["3"],
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(11rem, 1fr))",
  },
  stat: {
    borderColor: { default: colors.border, ":hover": colors.ring },
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["1"],
    paddingBlock: spacing["3"],
    paddingInline: spacing["4"],
    backgroundColor: colors.background,
    color: colors.foreground,
    display: "flex",
    flexDirection: "column",
    textDecorationLine: "none",
  },
  number: {
    fontFamily: display.fontFamily,
    fontSize: "1.875rem",
    fontVariantNumeric: "tabular-nums lining-nums",
    fontVariationSettings: '"opsz" 36',
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: "2.25rem",
  },
  waits: {
    color: tones.waits,
  },
  aside: {
    color: colors.mutedForeground,
  },
  list: {
    columnGap: spacing["4"],
    rowGap: spacing["1"],
    display: "flex",
    flexWrap: "wrap",
  },
  plain: {
    textDecorationLine: "none",
  },
});
