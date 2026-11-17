import type { LogEntry } from "../api.ts";
import { Fields, Load, Ops, Page, PagedTable, Ref, Section } from "../components.tsx";
import { observationsOf } from "../format.ts";
import { useLoad, useNames, usePages } from "../hooks.ts";

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
          { title: "Actor", cell: (entry) => <code>{entry.changeset.actor}</code> },
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
          { title: "Reason", cell: (entry) => entry.changeset.reason ?? "-" },
        ]}
        rowKey={(entry) => entry.seq}
        empty="No changeset has been committed."
      />
    </Page>
  );
}

function Entry({ entry }: { entry: LogEntry }) {
  const names = useNames(observationsOf(entry.changeset.ops));
  return (
    <>
      <Fields
        rows={[
          ["Changeset", <code className="id whole">{entry.changeset.id}</code>],
          ["Actor", <code>{entry.changeset.actor}</code>],
          ["Reason", entry.changeset.reason ? <span className="reason">“{entry.changeset.reason}”</span> : null],
          [
            "Drafted by",
            entry.changeset.run ? (
              <Ref kind="run" id={entry.changeset.run} label={`reconcile run ${entry.changeset.run}`} />
            ) : null,
          ],
          ["Written against", `log position ${entry.changeset.base}`],
          ["Content hash", <code>{entry.content_hash}</code>],
        ]}
      />
      <Section title={`Operations (${entry.changeset.ops.length})`}>
        <Ops ops={entry.changeset.ops} names={names} />
      </Section>
    </>
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
    <Page title={`Log position ${id}`}>
      <Load state={entry}>
        {(entry) =>
          entry === null ? (
            <p className="notice failure" role="alert">
              The log has no position {id}.
            </p>
          ) : (
            <Entry entry={entry} />
          )
        }
      </Load>
    </Page>
  );
}
