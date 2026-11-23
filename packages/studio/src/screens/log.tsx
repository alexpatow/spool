import { getRouteApi } from "@tanstack/react-router";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done, Would } from "../act.tsx";
import type { LogEntry, LogSort } from "../api.ts";
import { Code, Field, Fields, Load, Notice, Page, Reason, Section, Text } from "../components/layout.tsx";
import { PagedTable } from "../components/table.tsx";
import { Id, Ops, Ref } from "../components/refs.tsx";
import { sortingOf, useNarrow } from "../components/filters.tsx";
import { LogIcon, UndoIcon } from "../icons.tsx";
import { keys, stale, useOpNames, useRead, useReadPages } from "../queries.ts";
import { Who } from "../who.tsx";

/** The changesets of one page of the log. */
const PAGE = 100;

const list = getRouteApi("/log/");
const one = getRouteApi("/log/$id");

export function Log() {
  const sorting = sortingOf(list.useSearch(), useNarrow(), { by: "seq", order: "desc" });
  const filters = { limit: PAGE, sort: sorting.by as LogSort, order: sorting.order };
  const pages = useReadPages(keys.log.list(filters), async (client, after) => {
    const page = await client.changesets({ ...filters, after: after as number | undefined });
    // A sorted log says nothing of what is after a page, and a full page may have more.
    const last = page.changesets.at(-1);
    return { items: page.changesets, next: last !== undefined && page.changesets.length === PAGE ? last.seq : null };
  });
  return (
    <Page title="Log">
      <PagedTable
        pages={pages}
        sorting={sorting}
        columns={[
          { title: "Position", cell: (entry) => <Ref kind="log" id={entry.seq} />, sort: "seq", first: "desc" },
          { title: "Actor", cell: (entry) => <Who actor={entry.changeset.actor} />, sort: "actor" },
          { title: "Operations", cell: (entry) => entry.changeset.ops.length, number: true, sort: "operations" },
          {
            title: "Run",
            sort: "run",
            first: "desc",
            cell: (entry) =>
              entry.changeset.run ? (
                <Ref kind="run" id={entry.changeset.run} label={`reconcile run ${entry.changeset.run}`} />
              ) : (
                "-"
              ),
          },
          { title: "Started by", cell: (entry) => (entry.started_by ? <Who actor={entry.started_by} /> : "-") },
          { title: "Reason", cell: (entry) => entry.changeset.reason ?? "-", wrap: true, sort: "reason" },
        ]}
        rowKey={(entry) => entry.seq}
        empty="No changesets."
      />
    </Page>
  );
}

function Entry({ entry }: { entry: LogEntry }) {
  const names = useOpNames(entry.changeset.ops);
  return (
    <>
      <Fields>
        <Field label="Changeset">
          <Id value={entry.changeset.id} />
        </Field>
        <Field label="Actor">
          <Who actor={entry.changeset.actor} />
        </Field>
        <Field label="Reason">{entry.changeset.reason ? <Reason>{entry.changeset.reason}</Reason> : null}</Field>
        <Field label="Run">
          {entry.changeset.run ? (
            <Ref kind="run" id={entry.changeset.run} label={`reconcile run ${entry.changeset.run}`} />
          ) : null}
        </Field>
        <Field label="Started by">{entry.started_by ? <Who actor={entry.started_by} /> : null}</Field>
        <Field label="Base">{`log position ${entry.changeset.base}`}</Field>
        <Field label="Content hash">
          <Code>{entry.content_hash}</Code>
        </Field>
      </Fields>
      <Section title="Operations" count={entry.changeset.ops.length}>
        <Ops ops={entry.changeset.ops} names={names} />
      </Section>
      <Undo seq={entry.seq} />
    </>
  );
}

/**
 * Takes the changeset back with a new one. The engine writes it and checks
 * it against what the store holds now, and can be asked to show it first.
 */
function Undo({ seq }: { seq: number }) {
  const [done, setDone] = useState<ReactNode>(null);
  return (
    <Section title="Undo" about="Commits a new changeset that reverses this one.">
      <Done>{done}</Done>
      <div>
        <Act
          stale={stale.store}
          label="Undo"
          icon={<UndoIcon />}
          title={`Undo log position ${seq}`}
          why="Reason"
          look={{
            label: "Preview",
            act: async (client, { reason }) => {
              const { changeset } = await client.undo(seq, { reason, commit: false });
              return (
                <>
                  <Text>Would commit:</Text>
                  <Would ops={changeset.ops} />
                </>
              );
            },
          }}
          act={async (client, { reason }) => {
            const { committed } = await client.undo(seq, { reason });
            return committed ? (
              <>
                Undone at <At committed={committed} />.
              </>
            ) : (
              "Undone."
            );
          }}
          done={setDone}
        />
      </div>
    </Section>
  );
}

export function Changeset() {
  const { id } = one.useParams();
  const seq = Number(id);
  const entry = useRead(keys.log.one(id), async (client): Promise<LogEntry | null> => {
    if (!Number.isInteger(seq) || seq < 1) return null;
    const { changesets } = await client.changesets({ after: seq - 1, limit: 1 });
    return changesets[0]?.seq === seq ? changesets[0] : null;
  });
  return (
    <Page
      title={`Log position ${id}`}
      kind={
        <>
          <LogIcon /> Changeset
        </>
      }
    >
      <Load state={entry}>
        {(entry) =>
          entry === null ? (
            <Notice tone="bad" role="alert">
              No log position {id}.
            </Notice>
          ) : (
            <Entry entry={entry} />
          )
        }
      </Load>
    </Page>
  );
}
