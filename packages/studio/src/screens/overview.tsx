import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import type { Info, RunRecord, TypeInfo } from "../api.ts";
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
  Section,
  Source,
  Table,
  When,
  Word,
} from "../components.tsx";
import { count } from "../format.ts";
import { useLoad } from "../hooks.ts";
import { href } from "../router.ts";
import { colors, radius, spacing, typography } from "../styles/shelf/tokens.stylex";
import { tones } from "../styles/tones.stylex";

/** How many of the latest runs the overview lists. */
const RUNS = 5;

export function Overview() {
  const loaded = useLoad(async (client) => {
    const [info, runs] = await Promise.all([client.info(), client.runs({ limit: RUNS })]);
    return { info, runs };
  });
  return (
    <Page title="Overview" about="What this server holds, where it came from, and what waits for someone.">
      <Load state={loaded}>{({ info, runs }) => <Store info={info} runs={runs} />}</Load>
    </Page>
  );
}

function Store({ info, runs }: { info: Info; runs: RunRecord[] }) {
  const { store } = info;
  return (
    <>
      <Waiting types={store.entity_types} />
      {store.entity_types.length === 0 ? <Quiet>The project declares no entity type.</Quiet> : null}
      {store.entity_types.map((type) => (
        <Section key={type.name} title={`Entity type ${type.name}`}>
          <div {...stylex.props(styles.stats)}>
            <Stat
              number={type.identities}
              label={type.identities === 1 ? "identity" : "identities"}
              address={href(["identities"], { type: type.name })}
            />
            <Stat
              number={type.observations}
              label={type.observations === 1 ? "record" : "records"}
              address={href(["records"], { type: type.name })}
            />
            <Stat
              number={type.unassigned}
              label="without an identity"
              address={href(["records"], { type: type.name, unassigned: true })}
              waits={type.unassigned > 0}
            />
            <Stat
              number={type.open_review_items}
              label={type.open_review_items === 1 ? "review item open" : "review items open"}
              address={href(["review"], { type: type.name })}
              waits={type.open_review_items > 0}
            />
          </div>
          <Table
            label={`Where the ${type.name} records came from`}
            columns={[
              {
                title: "Source",
                cell: (source) => (
                  <a
                    href={href(["records"], { type: type.name, source: source.source })}
                    {...stylex.props(styles.plain)}
                  >
                    <Source name={source.source} />
                  </a>
                ),
              },
              { title: "Records", cell: (source) => source.observations, number: true },
              {
                title: "Without an identity",
                cell: (source) =>
                  source.unassigned > 0 ? (
                    <LinkTo path={["records"]} query={{ type: type.name, source: source.source, unassigned: true }}>
                      {source.unassigned}
                    </LinkTo>
                  ) : (
                    0
                  ),
                number: true,
              },
              { title: "Last ingested", cell: (source) => <When at={source.last_ingested_at} /> },
            ]}
            rows={type.sources}
            rowKey={(source) => source.source}
            empty={`Nothing has been ingested as ${type.name} yet.`}
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
      <Section title="Latest runs" actions={<Onward path={["runs"]}>All runs</Onward>}>
        <Table
          label="Latest runs"
          columns={[
            { title: "Run", cell: (run) => <Ref kind="run" id={run.run} label={`Run ${run.run}`} /> },
            { title: "Kind", cell: (run) => run.kind },
            { title: "Status", cell: (run) => <Word value={run.status} /> },
            {
              title: "About",
              cell: (run) =>
                [run.entity_type, run.strategy?.name, run.extractor?.name].filter((part) => part).join(", "),
            },
            { title: "Started", cell: (run) => <When at={run.started_at} /> },
          ]}
          rows={runs}
          rowKey={(run) => run.run}
          empty="Nothing has run yet."
        />
      </Section>
      {(store.extractors ?? []).length > 0 ? (
        <Section title="Extractors">
          <Table
            columns={[
              { title: "Extractor", cell: (extractor) => <Code>{extractor.name}</Code> },
              { title: "Runs with", cell: (extractor) => extractor.runtime },
              { title: "File", cell: (extractor) => <Code>{extractor.file}</Code> },
            ]}
            rows={store.extractors ?? []}
            rowKey={(extractor) => extractor.name}
            empty="The project has no extractor."
          />
        </Section>
      ) : null}
      <Section title="Store">
        <Fields
          rows={[
            ["API", `${info.api}, engine ${store.engine_version}, store format ${store.format_version}`],
            ["Manifest", store.manifest_hash ? <Code>{store.manifest_hash}</Code> : "none applied"],
            ["Head", <LinkTo path={["log"]}>log position {store.head}</LinkTo>],
            ["Documents", <LinkTo path={["documents"]}>{store.documents ?? 0}</LinkTo>],
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
          {count(type.open_review_items, "review item")} of {type.name}{" "}
          {type.open_review_items === 1 ? "waits" : "wait"} for a decision.{" "}
          <Onward path={["review"]} query={{ type: types.length > 1 ? type.name : undefined }}>
            Review
          </Onward>
        </span>,
      );
    }
    if (type.open_draft) {
      lines.push(
        <span key={`${type.name}-draft`}>
          A run left a draft of {type.name} changes for approval.{" "}
          <Onward path={["drafts", type.open_draft]}>Open the draft</Onward>
        </span>,
      );
    }
  }
  if (lines.length === 0) {
    return types.length > 0 ? <Notice tone="good">Nothing waits for a decision.</Notice> : null;
  }
  return <Notice tone="waits">{lines}</Notice>;
}

function Stat({
  number,
  label,
  address,
  waits = false,
}: {
  number: number;
  label: string;
  address: string;
  waits?: boolean;
}) {
  return (
    <a href={address} data-part="stat" data-waits={waits ? "" : undefined} {...stylex.props(styles.stat)}>
      <strong {...stylex.props(styles.number, waits && styles.waits)}>{number.toLocaleString("en-GB")}</strong>
      <span {...stylex.props(styles.aside)}>{label}</span>
    </a>
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
    fontSize: "1.5rem",
    fontVariantNumeric: "tabular-nums",
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.02em",
    lineHeight: "2rem",
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
