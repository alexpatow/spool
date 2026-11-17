import * as stylex from "@stylexjs/stylex";
import { useMemo, useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done } from "../act.tsx";
import type { ConstraintRecord, HistoryEntry, IdentitySummary, IdentityView, ObservationRecord } from "../api.ts";
import {
  Code,
  ExplainTicked,
  Filters,
  Flag,
  Id,
  Labelled,
  Load,
  Notice,
  Ops,
  Page,
  PagedTable,
  Panel,
  Quiet,
  Reason,
  Ref,
  Row,
  Section,
  Source,
  Sources,
  Table,
  Text,
  When,
  Word,
  message,
  useTicks,
} from "../components.tsx";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import * as ShelfTable from "../components/ui/table";
import { facts, latest, sourcesOf } from "../facts.ts";
import type { Fact } from "../facts.ts";
import { findIdentity } from "../find.ts";
import { count, identitiesOf, observationName, observationsOf, short, tail } from "../format.ts";
import { useLabels, useLoad, useNames, usePages, useStudio } from "../hooks.ts";
import { IdentityIcon, MergeIcon, SplitIcon } from "../icons.tsx";
import type { Route } from "../router.ts";
import { colors, spacing, typography } from "../styles/shelf/tokens.stylex";

/** What to call an identity: what most of its records call it, or its ID when none of them says. */
export function identityName(identity: Pick<IdentitySummary, "id" | "label">): string {
  return identity.label ?? short(identity.id);
}

export function Identities({ route, types }: { route: Route; types: string[] }) {
  const { type, q } = route.query;
  const { info } = useStudio();
  const pages = usePages(async (client, after) => {
    const page = await client.identities({ entity_type: type, q, after: after as string | undefined });
    return { items: page.identities, next: page.next };
  });
  const total = info.store.entity_types
    .filter((entity) => type === undefined || entity.name === type)
    .reduce((sum, entity) => sum + entity.identities, 0);
  return (
    <Page title="Identities" about={q ? undefined : count(total, "identity")}>
      <Filters
        path={["identities"]}
        query={route.query}
        search={{ name: "q", label: "Search", placeholder: "Any value in its records" }}
        selects={[{ name: "type", label: "Entity type", choices: types.map((value) => ({ value })) }]}
      />
      <PagedTable
        pages={pages}
        label="Identities"
        columns={[
          {
            title: "Name",
            cell: (identity) => (
              <Ref
                kind="identity"
                id={identity.id}
                label={<span {...stylex.props(styles.name)}>{identityName(identity)}</span>}
              />
            ),
            wrap: true,
          },
          { title: "Entity type", cell: (identity) => identity.entity_type },
          { title: "Records", cell: (identity) => identity.members, number: true },
          { title: "Sources", cell: (identity) => <Sources sources={identity.sources} />, wrap: true },
          { title: "ID", cell: (identity) => <Ref kind="identity" id={identity.id} /> },
        ]}
        rowKey={(identity) => identity.id}
        empty={q ? `No identities match “${q}”.` : "No identities."}
      />
    </Page>
  );
}

/** The most records of one identity that are read to show what they say. */
const MOST_RECORDS = 1000;

interface Loaded {
  /** Where the log ended before the rest was read: what a change made from this screen is written against. */
  head: number;
  view: IdentityView;
  /** Its members in full, as many as are read. */
  records: ObservationRecord[];
  constraints: ConstraintRecord[];
  history: HistoryEntry[];
}

export function Identity({ id }: { id: string }) {
  const identity = useLoad(async (client): Promise<Loaded> => {
    const { store } = await client.info();
    const [view, history] = await Promise.all([client.identity(id), client.identityHistory(id)]);
    // A merged identity answers with the members of the one that stands for
    // it, and the constraints are those on the members shown.
    const [constraints, page] = await Promise.all([
      client.constraints({ identity: view.current }),
      client.observations({ identity: view.current, limit: MOST_RECORDS }),
    ]);
    return { head: store.head, view, records: page.observations, constraints, history };
  });
  const view = identity.status === "ready" ? identity.data.view : null;
  return (
    <Page
      title={view ? identityName(view) : "Identity"}
      kind={
        <>
          <IdentityIcon /> Identity{view ? ` · ${view.entity_type}` : ""}
        </>
      }
      about={<Id value={id} />}
    >
      <Load state={identity}>{(loaded) => <IdentityScreen {...loaded} />}</Load>
    </Page>
  );
}

function IdentityScreen({ head, view, records, constraints, history }: Loaded) {
  const { chosen, tick } = useTicks();
  // Only an identity that stands can be changed. One that was merged away
  // shows the members of another.
  const stands = view.current === view.id;
  const byId = useMemo(() => new Map(records.map((record) => [record.id, record])), [records]);
  const members = useMemo(() => new Map(records.map((record) => [record.id, observationName(record)])), [records]);
  // The constraints and the history also name observations of other
  // identities, and ones that have left this one since.
  const names = useNames(
    [
      ...constraints.flatMap((record) => [record.a, record.b]),
      ...history.flatMap((entry) => observationsOf(entry.ops)),
    ],
    members,
  );
  // Its history names the identities it was merged with or parted from.
  const labels = useLabels(history.flatMap((entry) => identitiesOf(entry.ops)));
  const called = useMemo(
    () => new Map([...names, ...[...labels].map(([id, label]) => [id, `${label} (…${tail(id)})`] as const)]),
    [names, labels],
  );
  const placed = useMemo(() => new Map(history.map((entry) => [entry.seq, entry])), [history]);
  const current = latest(records);
  const sources = sourcesOf(records);
  return (
    <>
      {!stands ? (
        <Notice tone="waits">
          <span>
            Merged into <Ref kind="identity" id={view.current} />. Showing its records.
          </span>
        </Notice>
      ) : null}
      <Row>
        <span>
          {count(view.members.length, "record")} from {count(sources.length, "source")}
        </span>
        <Sources sources={sources} />
        <Quiet>
          · created at <Ref kind="log" id={view.created} label={`log position ${view.created}`} />
        </Quiet>
      </Row>
      <Section
        title="Fields"
        about={
          current.length < records.length
            ? `${count(records.length - current.length, "superseded version")} not counted.`
            : undefined
        }
      >
        <Facts facts={facts(current)} of={current.length} />
        {view.members.length > records.length ? (
          <Quiet>
            From the first {records.length} of {view.members.length} records.
          </Quiet>
        ) : null}
      </Section>
      <Section
        title="Records"
        count={view.members.length}
        actions={view.members.length > 1 ? <ExplainTicked chosen={chosen} /> : null}
      >
        <Table
          label="Records"
          columns={[
            { title: "", cell: (member) => tick(member.observation) },
            {
              title: "Record",
              cell: (member) => {
                const record = byId.get(member.observation);
                return (
                  <span {...stylex.props(styles.record)}>
                    <Ref
                      kind="observation"
                      id={member.observation}
                      label={record?.label ?? names.get(member.observation)}
                    />
                    {record?.superseded_by ? <Word value="replaced" label="superseded" /> : null}
                  </span>
                );
              },
              wrap: true,
            },
            { title: "Source", cell: (member) => <Source name={member.source} /> },
            {
              title: "Key",
              cell: (member) => {
                const record = byId.get(member.observation);
                return <Code>{record ? keyOf(record) : (member.key ?? "-")}</Code>;
              },
            },
            {
              title: "Ingested",
              cell: (member) => {
                const record = byId.get(member.observation);
                return record ? (
                  <>
                    <When at={record.ingested_at} /> by{" "}
                    <Ref kind="run" id={record.ingest_run} label={`run ${record.ingest_run}`} />
                  </>
                ) : null;
              },
            },
            {
              title: "Assigned",
              cell: (member) => <Placed since={member.since} entry={placed.get(member.since)} />,
              wrap: true,
            },
          ]}
          rows={view.members}
          rowKey={(member) => member.observation}
          empty="No records."
        />
      </Section>
      {stands && view.members.length > 1 ? <Split view={view} head={head} chosen={chosen} /> : null}
      {stands ? <Merge view={view} head={head} /> : null}
      {constraints.length > 0 ? (
        <Section title="Constraints">
          <Table
            columns={[
              { title: "Constraint", cell: (record) => <Word value={record.kind} /> },
              {
                title: "Between",
                cell: (record) => <Ref kind="observation" id={record.a} label={names.get(record.a)} />,
              },
              { title: "And", cell: (record) => <Ref kind="observation" id={record.b} label={names.get(record.b)} /> },
              {
                title: "Since",
                cell: (record) => <Ref kind="log" id={record.from} label={`log position ${record.from}`} />,
              },
            ]}
            rows={constraints}
            rowKey={(record) => `${record.kind}-${record.a}-${record.b}-${record.from}`}
            empty="No constraints."
          />
        </Section>
      ) : null}
      <Section title="History">
        <History history={history} names={called} />
      </Section>
    </>
  );
}

/** What a record is known by in its source: its key, or where in a document it was found. */
function keyOf(record: ObservationRecord): string {
  const name = observationName(record);
  return name.startsWith(`${record.source}:`) ? name.slice(record.source.length + 1) : name;
}

/** The changeset that put a record in the identity: where it is in the log, and who made it. */
function Placed({ since, entry }: { since: number; entry?: HistoryEntry }) {
  return (
    <span>
      <Ref kind="log" id={since} label={`log position ${since}`} />
      {entry ? (
        <span {...stylex.props(styles.aside)}>
          {" "}
          by {entry.run ? <Ref kind="run" id={entry.run} label={`run ${entry.run}`} /> : <Code>{entry.actor}</Code>}
          {entry.reason ? (
            <>
              {" "}
              <Reason>{entry.reason}</Reason>
            </>
          ) : null}
        </span>
      ) : null}
    </span>
  );
}

/** The most records that are named beside a value. Past that, their sources are counted. */
const MOST_SAYING = 3;

/**
 * What a set of records says, a field at a time: each value some record
 * holds, with the records that hold it. Where they disagree the field says
 * so, and every value is there.
 */
export function Facts({ facts, of }: { facts: Fact[]; of: number }) {
  if (facts.length === 0) {
    return <Quiet>No fields.</Quiet>;
  }
  return (
    <Panel flush>
      <ShelfTable.Root aria-label="Fields">
        <ShelfTable.Header>
          <ShelfTable.Row style={styles.headRow}>
            <ShelfTable.Head style={styles.head}>Field</ShelfTable.Head>
            <ShelfTable.Head style={styles.head}>Value</ShelfTable.Head>
            <ShelfTable.Head style={styles.head}>Records</ShelfTable.Head>
          </ShelfTable.Row>
        </ShelfTable.Header>
        <ShelfTable.Body>
          {facts.flatMap((fact) =>
            fact.values.map((said, index) => (
              <ShelfTable.Row
                key={`${fact.field}-${index}`}
                data-field={fact.field}
                style={[styles.factRow, index === fact.values.length - 1 && styles.factEnd]}
              >
                {index === 0 ? (
                  <ShelfTable.Cell rowSpan={fact.values.length} style={[styles.cell, styles.fieldName]}>
                    <span {...stylex.props(styles.fieldLine)}>
                      <Code>{fact.field}</Code>
                      {fact.agreed ? null : (
                        <Word value="differs" tone="waits" label={`${fact.values.length} values`} />
                      )}
                    </span>
                  </ShelfTable.Cell>
                ) : null}
                <ShelfTable.Cell style={[styles.cell, styles.factValue]}>{said.value}</ShelfTable.Cell>
                <ShelfTable.Cell style={[styles.cell, styles.saying]}>
                  <span {...stylex.props(styles.sayers)}>
                    {said.records.length === of && of > MOST_SAYING ? (
                      <span {...stylex.props(styles.aside)}>all {of} records</span>
                    ) : said.records.length <= MOST_SAYING ? (
                      said.records.map((record) => <Sayer key={record.id} record={record} />)
                    ) : (
                      <>
                        <span {...stylex.props(styles.aside)}>{said.records.length} records</span>
                        <Sources sources={sourcesOf(said.records)} />
                      </>
                    )}
                  </span>
                </ShelfTable.Cell>
              </ShelfTable.Row>
            )),
          )}
        </ShelfTable.Body>
      </ShelfTable.Root>
    </Panel>
  );
}

/** A record that holds a value: its source, and its key as the link to it. */
function Sayer({ record }: { record: ObservationRecord }) {
  return (
    <span {...stylex.props(styles.sayer)}>
      <Source name={record.source} />
      <Ref kind="observation" id={record.id} label={keyOf(record)} />
    </span>
  );
}

/**
 * Divides the identity in two: the members that are ticked, and the rest.
 * The engine leaves the larger side under this ID, and the side that was
 * not ticked when they are equal, so the screen says which side that is
 * before the reviewer asks for it. That depends on every member, so the
 * split is based on where the log ended when this screen was read, and the
 * engine refuses it if the members changed after that. What moved is told
 * from the engine's answer afterwards.
 */
function Split({ view, head, chosen }: { view: IdentityView; head: number; chosen: string[] }) {
  const [done, setDone] = useState<ReactNode>(null);
  const [apart, setApart] = useState(true);
  // A member that was ticked and has left since is no longer one to split off.
  const ticked = chosen.filter((id) => view.members.some((member) => member.observation === id));
  const rest = view.members.length - ticked.length;
  const moves = (n: number) => (n === 1 ? "moves" : "move");
  const keeps = (n: number) => (n === 1 ? "keeps" : "keep");
  const others = rest === 1 ? "The other record" : `The other ${rest}`;
  // The engine leaves the larger side under this ID.
  const would =
    ticked.length > rest
      ? `${count(ticked.length, "selected record")} ${keeps(ticked.length)} this ID. ${others} ${moves(rest)} to a new identity.`
      : `${count(ticked.length, "selected record")} ${moves(ticked.length)} to a new identity. ${others} ${keeps(rest)} this ID.`;
  return (
    <Section title="Split">
      <Done>{done}</Done>
      {ticked.length === 0 || rest === 0 ? (
        <Quiet>Select some of the records above to split them off.</Quiet>
      ) : (
        <Row>
          <Text>{would}</Text>
          <Act
            label="Split"
            icon={<SplitIcon />}
            about={would}
            who
            why="Reason"
            act={async (client, { actor, reason }) => {
              const report = await client.split(view.id, {
                actor,
                reason,
                base: head,
                observations: ticked,
                keep_apart: apart,
              });
              return (
                <>
                  Split at <At committed={report.committed} />: {count(report.moved.length, "record")} moved to{" "}
                  <Ref kind="identity" id={report.identity} />.
                </>
              );
            }}
            done={setDone}
          >
            <Flag label="Keep the two sides apart in later runs" checked={apart} onChange={setApart} />
          </Act>
        </Row>
      )}
    </Section>
  );
}

/** The most records of the other identity that are named before the merge is made. */
const MOST_NAMED = 5;

/** Another identity as it was looked up, and where the log ended on this screen then. */
interface Looked {
  view: IdentityView;
  head: number;
}

/**
 * Merges another identity into this one. The other is looked up and shown
 * first, so that the reviewer sees what is about to be joined.
 *
 * The merge is written against where the log ended when this screen was
 * read, and the other identity is looked up after that, so the engine
 * rejects the merge if either changed after the reviewer saw it. Reading
 * the screen again moves that position on, and what was looked up before
 * it is then older than what the merge would be written against, so it is
 * taken away and has to be looked up again.
 */
function Merge({ view, head }: { view: IdentityView; head: number }) {
  const { client } = useStudio();
  const [done, setDone] = useState<ReactNode>(null);
  const [looked, setOther] = useState<Looked | null>(null);
  const other = looked?.head === head ? looked.view : null;
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <Section title="Merge">
      <Done>{done}</Done>
      <form
        {...stylex.props(styles.lookup)}
        onSubmit={(event) => {
          event.preventDefault();
          const reference = String(new FormData(event.currentTarget).get("other") ?? "").trim();
          if (reference === "" || busy) return;
          setBusy(true);
          setProblem(null);
          setOther(null);
          findIdentity(client, reference).then(
            (found) => {
              setBusy(false);
              if (found === null) setProblem(`No identity found for ${reference}.`);
              else if (found.id === view.id) setProblem("That is this identity.");
              else setOther({ view: found, head });
            },
            (error: unknown) => {
              setBusy(false);
              setProblem(message(error));
            },
          );
        }}
      >
        <Labelled label="Identity to merge into this one">
          <Input
            name="other"
            placeholder="ID, or source:key of one of its records"
            spellCheck={false}
            autoComplete="off"
            style={styles.lookupBox}
          />
        </Labelled>
        <Button type="submit" variant="outline" disabled={busy}>
          Find
        </Button>
      </form>
      {problem ? (
        <Notice tone="bad" role="alert">
          {problem}
        </Notice>
      ) : null}
      {looked && !other ? <Notice tone="waits">The store changed since. Find it again.</Notice> : null}
      {other ? (
        <Panel>
          <div {...stylex.props(styles.other)}>
            <Text>
              <Ref kind="identity" id={other.id} label={identityName(other)} /> ·{" "}
              {count(other.members.length, "record")}:{" "}
              {other.members
                .slice(0, MOST_NAMED)
                .map((member) => (member.key ? `${member.source}:${member.key}` : member.source))
                .join(", ")}
              {other.members.length > MOST_NAMED ? ` and ${other.members.length - MOST_NAMED} more` : ""}
            </Text>
            <Act
              label="Merge"
              icon={<MergeIcon />}
              variant="default"
              title={`Merge ${identityName(other)} into this identity`}
              about={`${count(other.members.length, "record")} ${other.members.length === 1 ? "moves" : "move"} here.`}
              who
              why="Reason"
              act={async (client, { actor, reason }) => {
                // Written against what this screen read: if the members of
                // either identity changed since, the engine rejects the merge.
                const committed = await client.commit({
                  actor,
                  reason,
                  base: head,
                  ops: [{ op: "merge", from: other.id, into: view.id }],
                });
                return (
                  <>
                    Merged at <At committed={committed} />.
                  </>
                );
              }}
              done={(said) => {
                setOther(null);
                setDone(said);
              }}
            />
          </div>
        </Panel>
      ) : null}
    </Section>
  );
}

export function History({ history, names }: { history: HistoryEntry[]; names: Map<string, string> }) {
  if (history.length === 0) return <Quiet>No history.</Quiet>;
  return (
    <ol data-part="history" {...stylex.props(styles.history)}>
      {history.map((entry) => (
        <li key={entry.seq} {...stylex.props(styles.entry)}>
          <span {...stylex.props(styles.entryMark)} />
          <div {...stylex.props(styles.entryBody)}>
            <p {...stylex.props(styles.entryHead)}>
              <Ref kind="log" id={entry.seq} label={`Log position ${entry.seq}`} />
              <span {...stylex.props(styles.aside)}>
                {" "}
                <When at={entry.committed_at} />
              </span>{" "}
              by <Code>{entry.actor}</Code>
              {entry.run ? (
                <>
                  , drafted by <Ref kind="run" id={entry.run} label={`reconcile run ${entry.run}`} />
                </>
              ) : null}
              {entry.reason ? (
                <>
                  {" "}
                  <Reason>{entry.reason}</Reason>
                </>
              ) : null}
            </p>
            <Ops ops={entry.ops} names={names} />
          </div>
        </li>
      ))}
    </ol>
  );
}

const styles = stylex.create({
  name: {
    color: colors.foreground,
    fontWeight: typography.fontWeightMedium,
  },
  record: {
    gap: spacing["2"],
    alignItems: "center",
    display: "inline-flex",
    flexWrap: "wrap",
    fontWeight: typography.fontWeightMedium,
  },
  aside: {
    color: colors.mutedForeground,
  },
  headRow: {
    backgroundColor: colors.card,
  },
  head: {
    paddingInline: spacing["3"],
    fontSize: typography.fontSizeXs,
    height: "2.25rem",
  },
  cell: {
    paddingBlock: spacing["2"],
    paddingInline: spacing["3"],
    verticalAlign: "top",
  },
  factRow: {
    borderBottomWidth: 0,
  },
  factEnd: {
    borderBottomWidth: { default: 1, ":last-child": 0 },
  },
  fieldName: {
    width: "1%",
  },
  fieldLine: {
    gap: spacing["2"],
    alignItems: "center",
    display: "inline-flex",
  },
  factValue: {
    fontWeight: typography.fontWeightMedium,
    overflowWrap: "anywhere",
    whiteSpace: "normal",
  },
  saying: {
    whiteSpace: "normal",
  },
  sayers: {
    columnGap: spacing["3"],
    rowGap: spacing["1"],
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
  },
  sayer: {
    gap: spacing["1.5"],
    alignItems: "center",
    display: "inline-flex",
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
  },
  lookup: {
    gap: spacing["3"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
  },
  lookupBox: {
    width: "24rem",
    maxWidth: "100%",
  },
  other: {
    gap: spacing["3"],
    alignItems: "flex-start",
    display: "flex",
    flexDirection: "column",
  },
  history: {
    margin: 0,
    padding: 0,
    display: "flex",
    flexDirection: "column",
    listStyleType: "none",
  },
  entry: {
    gap: spacing["3"],
    display: "flex",
    paddingBottom: { default: spacing["4"], ":last-child": 0 },
    position: "relative",
    "::before": {
      backgroundColor: colors.border,
      content: "''",
      position: "absolute",
      bottom: 0,
      left: "0.25rem",
      top: "0.75rem",
      width: 1,
    },
  },
  entryMark: {
    borderColor: colors.background,
    borderRadius: "9999px",
    borderStyle: "solid",
    borderWidth: 2,
    backgroundColor: colors.mutedForeground,
    flexShrink: 0,
    marginTop: "0.3125rem",
    position: "relative",
    zIndex: 1,
    height: "0.625rem",
    width: "0.625rem",
    boxSizing: "border-box",
  },
  entryBody: {
    gap: spacing["1.5"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  entryHead: {
    margin: 0,
  },
});
