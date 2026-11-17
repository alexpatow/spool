import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done, Would } from "../act.tsx";
import type { LogEntry } from "../api.ts";
import { Code, Fields, Id, Load, Notice, Ops, Page, PagedTable, Reason, Ref, Section, Text } from "../components.tsx";
import { LogIcon, UndoIcon } from "../icons.tsx";
import { useLoad, useOpNames, usePages } from "../hooks.ts";

export function Log() {
  const pages = usePages(async (client, after) => {
    const page = await client.changesets({ after: after as number | undefined });
    const last = page.changesets.at(-1);
    return { items: page.changesets, next: last !== undefined && last.seq < page.head ? last.seq : null };
  });
  return (
    <Page title="Log" about="Every committed changeset, oldest first. Identity state is what the log adds up to.">
      <PagedTable
        pages={pages}
        columns={[
          { title: "Position", cell: (entry) => <Ref kind="log" id={entry.seq} /> },
          { title: "Actor", cell: (entry) => <Code>{entry.changeset.actor}</Code> },
          { title: "Operations", cell: (entry) => entry.changeset.ops.length, number: true },
          {
            title: "From",
            cell: (entry) =>
              entry.changeset.run ? (
                <Ref kind="run" id={entry.changeset.run} label={`reconcile run ${entry.changeset.run}`} />
              ) : (
                "-"
              ),
          },
          { title: "Reason", cell: (entry) => entry.changeset.reason ?? "-", wrap: true },
        ]}
        rowKey={(entry) => entry.seq}
        empty="No changeset has been committed."
      />
    </Page>
  );
}

function Entry({ entry }: { entry: LogEntry }) {
  const names = useOpNames(entry.changeset.ops);
  return (
    <>
      <Fields
        rows={[
          ["Changeset", <Id value={entry.changeset.id} />],
          ["Actor", <Code>{entry.changeset.actor}</Code>],
          ["Reason", entry.changeset.reason ? <Reason>{entry.changeset.reason}</Reason> : null],
          [
            "Drafted by",
            entry.changeset.run ? (
              <Ref kind="run" id={entry.changeset.run} label={`reconcile run ${entry.changeset.run}`} />
            ) : null,
          ],
          ["Written against", `log position ${entry.changeset.base}`],
          ["Content hash", <Code>{entry.content_hash}</Code>],
        ]}
      />
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
    <Section title="Undo it" about="Takes the changeset back with a new one. Nothing is removed from the log.">
      <Done>{done}</Done>
      <div>
        <Act
          label="Undo it"
          icon={<UndoIcon />}
          title={`Undo log position ${seq}`}
          about="The engine writes the changeset that takes this one back, and refuses it if it no longer holds."
          who
          why="Reason"
          look={{
            label: "Show what undoing it would do",
            act: async (client, { actor, reason }) => {
              const { changeset } = await client.undo(seq, { actor, reason, commit: false });
              return (
                <>
                  <Text>Undoing it would make this changeset, which holds as the store is now:</Text>
                  <Would ops={changeset.ops} />
                </>
              );
            },
          }}
          act={async (client, { actor, reason }) => {
            const { committed } = await client.undo(seq, { actor, reason });
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

export function Changeset({ id }: { id: string }) {
  const seq = Number(id);
  const entry = useLoad(async (client): Promise<LogEntry | null> => {
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
              The log has no position {id}.
            </Notice>
          ) : (
            <Entry entry={entry} />
          )
        }
      </Load>
    </Page>
  );
}
