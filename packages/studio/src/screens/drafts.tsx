import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done } from "../act.tsx";
import type { DraftRecord, DraftStatus, DraftSummary } from "../api.ts";
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
    <Page title="Drafts" about="Changesets a run left for review, with what became of each.">
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
                title: "From",
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
      <Load state={draft}>{(record) => <DraftScreen record={record} />}</Load>
    </Page>
  );
}

function DraftScreen({ record }: { record: DraftRecord }) {
  const { draft, changeset } = record;
  const names = useOpNames(changeset.ops);
  return (
    <>
      <Fields
        rows={[
          ["Entity type", draft.entity_type],
          ["Status", <Word value={draft.status} />],
          [
            "From",
            <>
              <Ref kind="run" id={draft.match_run} label={`match run ${draft.match_run}`} />,{" "}
              <Ref kind="run" id={draft.reconcile_run} label={`reconcile run ${draft.reconcile_run}`} />
            </>,
          ],
          ["Written against", `log position ${draft.base}`],
          [
            "Would place",
            `${count(draft.observations_assigned, "observation")}, ${count(draft.identities_created, "identity")} created`,
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
      <Section title="Operations" count={changeset.ops.length} about="What approving it would commit, in order.">
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
    <Section
      title="Decide"
      about={
        open ? "Approving commits its operations as they are. The engine refuses if they no longer hold." : undefined
      }
    >
      <Done>{done}</Done>
      {open ? (
        <Row>
          <Act
            label="Approve and commit"
            icon={<CheckIcon />}
            variant="default"
            about={`${count(draft.observations_assigned, "observation")} placed, ${count(draft.identities_created, "identity")} created. The changeset goes into the log, where it can be undone.`}
            act={async (client) => {
              const committed = await client.approveDraft(draft.id);
              return (
                <>
                  The draft was committed at <At committed={committed} />.
                </>
              );
            }}
            done={setDone}
          />
          <Act
            label="Reject"
            about="The draft is closed and nothing is committed. The next run examines its observations again."
            why="Why it is rejected"
            act={async (client, { reason }) => {
              await client.rejectDraft(draft.id, reason);
              return "The draft was rejected. The next run examines its observations again.";
            }}
            done={setDone}
          />
        </Row>
      ) : null}
    </Section>
  );
}
