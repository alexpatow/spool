import { useMemo, useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done } from "../act.tsx";
import type { ConstraintRecord, HistoryEntry, IdentityView } from "../api.ts";
import {
  ExplainTicked,
  Fields,
  Filters,
  Id,
  Load,
  Ops,
  Page,
  PagedTable,
  Ref,
  Section,
  Table,
  When,
  Word,
  message,
  useTicks,
} from "../components.tsx";
import { findIdentity } from "../find.ts";
import { count, named, observationsOf } from "../format.ts";
import { useLoad, useNames, usePages, useStudio } from "../hooks.ts";
import type { Route } from "../router.ts";

export function Identities({ route, types }: { route: Route; types: string[] }) {
  const { type } = route.query;
  const pages = usePages(async (client, after) => {
    const page = await client.identities({ entity_type: type, after: after as string | undefined });
    return { items: page.identities, next: page.next };
  });
  return (
    <Page title="Identities" about="The identities that stand now. One that was merged away is found by its ID.">
      <Filters
        path={["identities"]}
        query={route.query}
        selects={[{ name: "type", label: "Entity type", choices: types.map((value) => ({ value })) }]}
      />
      <PagedTable
        pages={pages}
        columns={[
          { title: "Identity", cell: (identity) => <Ref kind="identity" id={identity.id} /> },
          { title: "Entity type", cell: (identity) => identity.entity_type },
          { title: "Members", cell: (identity) => identity.members, number: true },
          { title: "Created at", cell: (identity) => `log position ${identity.created}` },
        ]}
        rowKey={(identity) => identity.id}
        empty="No identities."
      />
    </Page>
  );
}

interface Loaded {
  /** Where the log ended before the rest was read: what a change made from this screen is written against. */
  head: number;
  view: IdentityView;
  constraints: ConstraintRecord[];
  history: HistoryEntry[];
}

export function Identity({ id }: { id: string }) {
  const identity = useLoad(async (client): Promise<Loaded> => {
    const { store } = await client.info();
    const [view, history] = await Promise.all([client.identity(id), client.identityHistory(id)]);
    // A merged identity answers with the members of the one that stands for
    // it, and the constraints are those on the members shown.
    const constraints = await client.constraints({ identity: view.current });
    return { head: store.head, view, constraints, history };
  });
  return (
    <Page title="Identity" about={<Id value={id} />}>
      <Load state={identity}>{(loaded) => <IdentityScreen {...loaded} />}</Load>
    </Page>
  );
}

/** The key the engine gives a mention: the ID of its document, then what was found and where. */
const MENTION_KEY = /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}#/;

function IdentityScreen({ head, view, constraints, history }: Loaded) {
  const { chosen, tick } = useTicks();
  // Only an identity that stands can be changed. One that was merged away
  // shows the members of another.
  const stands = view.current === view.id;
  // A mention's key says nothing to a reader, so those are read for the
  // name of their document.
  const mentions = useNames(
    view.members.filter((member) => MENTION_KEY.test(member.key ?? "")).map((member) => member.observation),
  );
  const members = useMemo(
    () =>
      new Map(
        view.members.map((member) => [
          member.observation,
          mentions.get(member.observation) ?? named(member.source, member.key),
        ]),
      ),
    [view, mentions],
  );
  // The constraints and the history also name observations of other
  // identities, and ones that have left this one since.
  const names = useNames(
    [
      ...constraints.flatMap((record) => [record.a, record.b]),
      ...history.flatMap((entry) => observationsOf(entry.ops)),
    ],
    members,
  );
  return (
    <>
      <Fields
        rows={[
          [
            "Merged into",
            view.current !== view.id ? (
              <>
                <Ref kind="identity" id={view.current} />. The members below are its members.
              </>
            ) : null,
          ],
          ["Entity type", view.entity_type],
          ["Created at", <Ref kind="log" id={view.created} label={`log position ${view.created}`} />],
        ]}
      />
      <Section title={`Members (${view.members.length})`}>
        <Table
          columns={[
            { title: "", cell: (member) => tick(member.observation) },
            {
              title: "Observation",
              cell: (member) => (
                <Ref kind="observation" id={member.observation} label={names.get(member.observation)} />
              ),
            },
            { title: "ID", cell: (member) => <Ref kind="observation" id={member.observation} /> },
            {
              title: "Since",
              cell: (member) => <Ref kind="log" id={member.since} label={`log position ${member.since}`} />,
            },
          ]}
          rows={view.members}
          rowKey={(member) => member.observation}
          empty="No members."
        />
        {view.members.length > 1 ? <ExplainTicked chosen={chosen} /> : null}
      </Section>
      {stands && view.members.length > 1 ? <Split view={view} chosen={chosen} /> : null}
      {stands ? <Merge view={view} head={head} /> : null}
      {constraints.length > 0 ? (
        <Section title="Constraints on its members">
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
        <History history={history} names={names} />
      </Section>
    </>
  );
}

/**
 * Divides the identity in two: the members that are ticked, and the rest.
 * The engine leaves the larger side under this ID, and the side that was
 * not ticked when they are equal, so the screen says which side that is
 * before the reviewer asks for it. What moved is told from the engine's
 * answer afterwards.
 */
function Split({ view, chosen }: { view: IdentityView; chosen: string[] }) {
  const [done, setDone] = useState<ReactNode>(null);
  // A member that was ticked and has left since is no longer one to split off.
  const ticked = chosen.filter((id) => view.members.some((member) => member.observation === id));
  const rest = view.members.length - ticked.length;
  return (
    <Section title="Split it">
      <Done>{done}</Done>
      {ticked.length === 0 || rest === 0 ? (
        <p className="quiet">
          Tick some of the members, and not all of them, to part them from the rest. The larger side stays here, and the
          other side moves to an identity of its own.
        </p>
      ) : (
        <>
          <p className="quiet">
            {ticked.length > rest
              ? `The ${ticked.length} ticked are the larger side, so they stay here, under this ID. ${
                  rest === 1 ? "The other member moves" : `The other ${rest} move`
                } to a new identity.`
              : `${ticked.length === 1 ? "The ticked member moves" : `The ${ticked.length} ticked move`} to a new identity. ${
                  rest === 1 ? "The other member stays" : `The other ${rest} stay`
                } here, under this ID.`}
          </p>
          <Act
            label={`Split the ${ticked.length} ticked from the rest`}
            who
            why="Reason"
            act={async (client, { actor, reason, entered }) => {
              const report = await client.split(view.id, {
                actor,
                reason,
                observations: ticked,
                keep_apart: entered.get("keep_apart") !== null,
              });
              return (
                <>
                  Split at <At committed={report.committed} />: {count(report.moved.length, "observation")} moved to the
                  new identity <Ref kind="identity" id={report.identity} />.
                </>
              );
            }}
            done={setDone}
          >
            <label className="flag">
              <input type="checkbox" name="keep_apart" defaultChecked />
              Keep the two sides apart from now on
            </label>
          </Act>
        </>
      )}
    </Section>
  );
}

/** The most members of the other identity that are named before the merge is made. */
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
    <Section title="Merge another identity into it">
      <Done>{done}</Done>
      <form
        className="filters"
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
              if (found === null) setProblem(`No identity is named ${reference}.`);
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
        <label>
          The other identity
          <input
            name="other"
            placeholder="its ID, or an observation of it as source:key"
            spellCheck={false}
            autoComplete="off"
            size={40}
          />
        </label>
        <button type="submit" disabled={busy}>
          Look it up
        </button>
        {problem ? (
          <p className="notice failure" role="alert">
            {problem}
          </p>
        ) : null}
      </form>
      {looked && !other ? (
        <p className="notice">The log moved on after that identity was looked up. Look it up again.</p>
      ) : null}
      {other ? (
        <>
          <p>
            <Ref kind="identity" id={other.id} /> is a {other.entity_type} with {count(other.members.length, "member")}:{" "}
            {other.members
              .slice(0, MOST_NAMED)
              .map((member) => named(member.source, member.key))
              .join(", ")}
            {other.members.length > MOST_NAMED ? ` and ${other.members.length - MOST_NAMED} more` : ""}. Merging moves
            them here, and this identity stands for that one from then on.
          </p>
          <Act
            label="Merge it into this one"
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
        </>
      ) : null}
    </Section>
  );
}

function History({ history, names }: { history: HistoryEntry[]; names: Map<string, string> }) {
  if (history.length === 0) return <p className="quiet">No changeset names it.</p>;
  return (
    <ol className="history">
      {history.map((entry) => (
        <li key={entry.seq}>
          <p>
            <Ref kind="log" id={entry.seq} label={`Log position ${entry.seq}`} />
            <span className="quiet">
              {" "}
              <When at={entry.committed_at} />
            </span>{" "}
            by <code>{entry.actor}</code>
            {entry.run ? (
              <>
                , drafted by <Ref kind="run" id={entry.run} label={`reconcile run ${entry.run}`} />
              </>
            ) : null}
          </p>
          {entry.reason ? <p className="reason">“{entry.reason}”</p> : null}
          <Ops ops={entry.ops} names={names} />
        </li>
      ))}
    </ol>
  );
}
