import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done } from "../act.tsx";
import type { DraftEntry, DraftStatus, DraftSummary } from "../api.ts";
import { Fields, Filters, Id, Load, Ops, Page, Reason, Ref, Row, Section, Table, When, Word } from "../components.tsx";
import { CheckIcon, DraftIcon } from "../icons.tsx";
import { count } from "../format.ts";
import { useLoad, useOpNames } from "../hooks.ts";
import type { Route } from "../router.ts";

const STATUSES: DraftStatus[] = ["open", "committed", "rejected", "superseded"];

export function Drafts({ route, types }: { route: Route; types: string[] }) {
  const { status, type } = route.query;
  const drafts = useLoad((client) => client.drafts({ status: status as DraftStatus | undefined, entity_type: type }));
  return (
    <Page title="Drafts">
      <Filters
        path={["drafts"]}
        query={route.query}
        selects={[
          { name: "status", label: "Status", choices: STATUSES.map((value) => ({ value })) },
          { name: "type", label: "Entity type", choices: types.map((value) => ({ value })) },
        ]}
      />
      <Load state={drafts}>
        {(drafts) => (
          <Table
            columns={[
              { title: "Draft", cell: (draft) => <Ref kind="draft" id={draft.id} /> },
              { title: "Entity type", cell: (draft) => draft.entity_type },
              { title: "Status", cell: (draft) => <Word value={draft.status} /> },
              { title: "Operations", cell: (draft) => draft.operations, number: true },
              {
                title: "Run",
                cell: (draft) => <Ref kind="run" id={draft.match_run} label={`match run ${draft.match_run}`} />,
              },
            ]}
            rows={drafts}
            rowKey={(draft) => draft.id}
            empty="No drafts."
          />
        )}
      </Load>
    </Page>
  );
}

export function Draft({ id }: { id: string }) {
  const draft = useLoad((client) => client.draft(id));
  return (
    <Page
      title="Draft"
      kind={
        <>
          <DraftIcon /> Draft
        </>
      }
      about={<Id value={id} />}
    >
      <Load state={draft}>{(entry) => <DraftScreen entry={entry} />}</Load>
    </Page>
  );
}

function DraftScreen({ entry }: { entry: DraftEntry }) {
  const { draft, changeset } = entry;
  const names = useOpNames(changeset.ops);
  return (
    <>
      <Fields
        rows={[
          ["Entity type", draft.entity_type],
          ["Status", <Word value={draft.status} />],
          [
            "Runs",
            <>
              <Ref kind="run" id={draft.match_run} label={`match run ${draft.match_run}`} />,{" "}
              <Ref kind="run" id={draft.reconcile_run} label={`reconcile run ${draft.reconcile_run}`} />
            </>,
          ],
          ["Base", `log position ${draft.base}`],
          [
            "Changes",
            `${count(draft.records_assigned, "record")} assigned, ${count(draft.identities_created, "identity")} created`,
          ],
          [
            "Committed at",
            draft.committed_seq ? (
              <Ref kind="log" id={draft.committed_seq} label={`log position ${draft.committed_seq}`} />
            ) : null,
          ],
          [
            "Superseded by",
            draft.superseded_by ? (
              <Ref kind="run" id={draft.superseded_by} label={`reconcile run ${draft.superseded_by}`} />
            ) : null,
          ],
          [
            "Rejected",
            draft.rejected_at ? (
              <>
                <When at={draft.rejected_at} />
                {draft.rejected_reason ? (
                  <>
                    {" "}
                    <Reason>{draft.rejected_reason}</Reason>
                  </>
                ) : null}
              </>
            ) : null,
          ],
        ]}
      />
      <Decide draft={draft} />
      <Section title="Operations" count={changeset.ops.length}>
        <Ops ops={changeset.ops} names={names} />
      </Section>
    </>
  );
}

/** Commits a draft that waits, or closes it. */
function Decide({ draft }: { draft: DraftSummary }) {
  const [done, setDone] = useState<ReactNode>(null);
  const open = draft.status === "open";
  if (!open && done === null) return null;
  return (
    <Section title="Decision">
      <Done>{done}</Done>
      {open ? (
        <Row>
          <Act
            label="Approve"
            icon={<CheckIcon />}
            variant="default"
            about={`Commits ${count(draft.operations, "operation")}: ${count(draft.records_assigned, "record")} assigned, ${count(draft.identities_created, "identity")} created.`}
            act={async (client) => {
              const committed = await client.approveDraft(draft.id);
              return (
                <>
                  Committed at <At committed={committed} />.
                </>
              );
            }}
            done={setDone}
          />
          <Act
            label="Reject"
            about="Nothing is committed. The next run examines its records again."
            why="Reason"
            act={async (client, { reason }) => {
              await client.rejectDraft(draft.id, reason);
              return "Rejected.";
            }}
            done={setDone}
          />
        </Row>
      ) : null}
    </Section>
  );
}
