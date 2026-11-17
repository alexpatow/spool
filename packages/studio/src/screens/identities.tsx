import { useMemo } from "react";

import type { ConstraintRecord, HistoryEntry, IdentityView } from "../api.ts";
import {
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
  usePair,
} from "../components.tsx";
import { named, observationsOf } from "../format.ts";
import { useLoad, useNames, usePages } from "../hooks.ts";
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
  view: IdentityView;
  constraints: ConstraintRecord[];
  history: HistoryEntry[];
}

export function Identity({ id }: { id: string }) {
  const identity = useLoad(async (client): Promise<Loaded> => {
    const [view, history] = await Promise.all([client.identity(id), client.identityHistory(id)]);
    // A merged identity answers with the members of the one that stands for
    // it, and the constraints are those on the members shown.
    const constraints = await client.constraints({ identity: view.current });
    return { view, constraints, history };
  });
  return (
    <Page title="Identity" about={<Id value={id} />}>
      <Load state={identity}>{(loaded) => <IdentityScreen {...loaded} />}</Load>
    </Page>
  );
}

/** The key the engine gives a mention: the ID of its document, then what was found and where. */
const MENTION_KEY = /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}#/;

function IdentityScreen({ view, constraints, history }: Loaded) {
  const pair = usePair();
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
            { title: "", cell: (member) => pair.tick(member.observation) },
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
        {view.members.length > 1 ? pair.explain : null}
      </Section>
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
