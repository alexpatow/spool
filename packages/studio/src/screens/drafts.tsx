import { getRouteApi } from "@tanstack/react-router";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done } from "../act.tsx";
import type { DraftEntry, DraftStatus, DraftSummary } from "../api.ts";
import { Field, Fields, Load, Page, Reason, Row, Section } from "../components/layout.tsx";
import { Table } from "../components/table.tsx";
import { Id, Ops, Ref, When, Word } from "../components/refs.tsx";
import { Filters, useNarrow } from "../components/filters.tsx";
import { CheckIcon, DraftIcon } from "../icons.tsx";
import { count } from "../format.ts";
import { useTypes } from "../hooks.ts";
import { keys, stale, useOpNames, useRead } from "../queries.ts";

const STATUSES: DraftStatus[] = ["open", "committed", "rejected", "superseded"];

const list = getRouteApi("/drafts/");
const one = getRouteApi("/drafts/$id");

export function Drafts() {
  const search = list.useSearch();
  const { status, type } = search;
  const types = useTypes();
  const narrow = useNarrow();
  const filters = { status: status as DraftStatus | undefined, entity_type: type };
  const drafts = useRead(keys.drafts.list(filters), (client) => client.drafts(filters));
  return (
    <Page title="Drafts">
      <Filters
        query={search}
        narrow={narrow}
        selects={[
          { name: "status", label: "Status", choices: STATUSES.map((value) => ({ value })) },
          { name: "type", label: "Entity type", choices: types.map((value) => ({ value })) },
        ]}
      />
      <Load state={drafts}>
        {(drafts) => (
          <Table
            columns={[
              { title: "Draft", value: (draft) => draft.id, cell: (draft) => <Ref kind="draft" id={draft.id} /> },
              { title: "Entity type", value: (draft) => draft.entity_type, cell: (draft) => draft.entity_type },
              { title: "Status", value: (draft) => draft.status, cell: (draft) => <Word value={draft.status} /> },
              {
                title: "Operations",
                value: (draft) => draft.operations,
                cell: (draft) => draft.operations,
                number: true,
              },
              {
                title: "Run",
                value: (draft) => draft.match_run,
                first: "desc",
                cell: (draft) => <Ref kind="run" id={draft.match_run} label={`match run ${draft.match_run}`} />,
              },
            ]}
            rows={drafts}
            initial={{ by: "Run", order: "desc" }}
            rowKey={(draft) => draft.id}
            empty="No drafts."
          />
        )}
      </Load>
    </Page>
  );
}

export function Draft() {
  const { id } = one.useParams();
  const draft = useRead(keys.drafts.one(id), (client) => client.draft(id));
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
      <Fields>
        <Field label="Entity type">{draft.entity_type}</Field>
        <Field label="Status">
          <Word value={draft.status} />
        </Field>
        <Field label="Runs">
          <>
            <Ref kind="run" id={draft.match_run} label={`match run ${draft.match_run}`} />,{" "}
            <Ref kind="run" id={draft.reconcile_run} label={`reconcile run ${draft.reconcile_run}`} />
          </>
        </Field>
        <Field label="Base">{`log position ${draft.base}`}</Field>
        <Field label="Changes">{`${count(draft.records_assigned, "record")} assigned, ${count(draft.identities_created, "identity")} created`}</Field>
        <Field label="Committed at">
          {draft.committed_seq ? (
            <Ref kind="log" id={draft.committed_seq} label={`log position ${draft.committed_seq}`} />
          ) : null}
        </Field>
        <Field label="Superseded by">
          {draft.superseded_by ? (
            <Ref kind="run" id={draft.superseded_by} label={`reconcile run ${draft.superseded_by}`} />
          ) : null}
        </Field>
        <Field label="Rejected">
          {draft.rejected_at ? (
            <>
              <When at={draft.rejected_at} />
              {draft.rejected_reason ? (
                <>
                  {" "}
                  <Reason>{draft.rejected_reason}</Reason>
                </>
              ) : null}
            </>
          ) : null}
        </Field>
      </Fields>
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
            stale={stale.store}
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
            stale={stale.store}
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
