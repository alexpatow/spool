import { getRouteApi } from "@tanstack/react-router";
import type { ReactNode } from "react";

import type { RunEntry, RunKind, RunSort, RunStatus } from "../api.ts";
import { Code, Fields, Lines, Load, Notice, Page, Section } from "../components/layout.tsx";
import { Table } from "../components/table.tsx";
import { Identity, Onward, Ref, When, Word } from "../components/refs.tsx";
import { Filters, sortingOf, useNarrow } from "../components/filters.tsx";
import { RunIcon } from "../icons.tsx";
import { count } from "../format.ts";
import { useTypes } from "../hooks.ts";
import { keys, useIdentityLabels, useRead } from "../queries.ts";
import { Who } from "../who.tsx";

const KINDS: RunKind[] = ["load", "extract", "match", "reconcile", "retract"];
const STATUSES: RunStatus[] = ["running", "open", "drafted", "succeeded", "failed"];
const STEP = 100;

const list = getRouteApi("/runs/");
const one = getRouteApi("/runs/$id");

export function Runs() {
  const search = list.useSearch();
  const { kind, status, type } = search;
  const limit = search.limit || STEP;
  const types = useTypes();
  const narrow = useNarrow();
  const sorting = sortingOf(search, narrow, { by: "run", order: "desc" });
  const filters = {
    kind: kind as RunKind | undefined,
    status: status as RunStatus | undefined,
    entity_type: type,
    limit,
    sort: sorting.by as RunSort,
    order: sorting.order,
  };
  const runs = useRead(keys.runs.list(filters), (client) => client.runs(filters));
  return (
    <Page title="Runs">
      <Filters
        query={search}
        narrow={narrow}
        selects={[
          { name: "kind", label: "Kind", choices: KINDS.map((value) => ({ value })) },
          { name: "status", label: "Status", choices: STATUSES.map((value) => ({ value })) },
          { name: "type", label: "Entity type", choices: types.map((value) => ({ value })) },
        ]}
      />
      <Load state={runs}>
        {(runs) => (
          <>
            <Table
              sorting={sorting}
              columns={[
                {
                  title: "Run",
                  cell: (run) => <Ref kind="run" id={run.run} label={`Run ${run.run}`} />,
                  sort: "run",
                  first: "desc",
                },
                { title: "Kind", cell: (run) => run.kind, sort: "kind" },
                { title: "Status", cell: (run) => <Word value={run.status} />, sort: "status" },
                {
                  title: "Details",
                  sort: "details",
                  cell: (run) =>
                    [run.entity_type, run.strategy?.name, run.extractor?.name].filter((part) => part).join(", "),
                },
                { title: "Started by", cell: (run) => <Who actor={run.started_by} />, sort: "started_by" },
                { title: "Started", cell: (run) => <When at={run.started_at} />, sort: "started", first: "desc" },
              ]}
              rows={runs}
              rowKey={(run) => run.run}
              empty="No runs."
            />
            {runs.length >= limit && limit < 1000 ? (
              <Onward to="/runs" search={{ ...search, limit: Math.min(1000, limit + STEP) }}>
                Show more
              </Onward>
            ) : null}
          </>
        )}
      </Load>
    </Page>
  );
}

export function Run() {
  const { id } = one.useParams();
  const run = useRead(keys.runs.one(id), (client) => client.run(Number(id)));
  return (
    <Page
      title={`Run ${id}`}
      kind={
        <>
          <RunIcon /> Run
        </>
      }
    >
      <Load state={run}>{(run) => <RunView run={run} />}</Load>
    </Page>
  );
}

/** What a run says of the `possible_split` review items it raised, or `null` when it raised none. */
function leftForReview(identities: number | undefined): ReactNode {
  if (!identities) return null;
  return (
    <>
      {count(identities, "identity")} with records that nothing holds together now.{" "}
      <Onward to="/review">review</Onward>
    </>
  );
}

function RunView({ run }: { run: RunEntry }) {
  const { load, extraction, resolution, retraction } = run;
  // The identities a strategy would merge or split are named by ID.
  const labels = useIdentityLabels([
    ...(resolution?.suggestions?.merges.flatMap((merge) => merge.identities) ?? []),
    ...(resolution?.suggestions?.splits.map((split) => split.identity) ?? []),
  ]);
  return (
    <>
      <Fields
        rows={[
          ["Kind", run.kind],
          ["Status", <Word value={run.status} />],
          ["Entity type", run.entity_type],
          [
            "Strategy",
            run.strategy ? (
              <>
                <Code>{run.strategy.name}</Code>
                {run.strategy.version ? ` version ${run.strategy.version}` : null}
              </>
            ) : null,
          ],
          [
            "Extractor",
            run.extractor ? (
              <>
                <Code>{run.extractor.name}</Code> version {run.extractor.version}
              </>
            ) : null,
          ],
          ["Started by", <Who actor={run.started_by} />],
          ["Started", <When at={run.started_at} />],
          ["Finished", run.finished_at ? <When at={run.finished_at} /> : null],
        ]}
      />
      {run.error ? (
        <Notice tone="bad" role="alert">
          {run.error}
        </Notice>
      ) : null}
      {load ? (
        <Section title="Load">
          <Fields
            rows={[
              [
                "Rows",
                `${load.received} received: ${load.inserted} new, ${load.duplicates} duplicates, ${load.superseded ?? 0} new versions`,
              ],
              [
                "Came back",
                load.restored
                  ? `${count(load.restored, "record")} the source had taken back, ${load.returned ?? 0} of them to the identity they left`
                  : null,
              ],
              ["Left for review", leftForReview(load.possible_splits)],
              [
                "Committed",
                load.committed ? (
                  <Ref kind="log" id={load.committed.seq} label={`log position ${load.committed.seq}`} />
                ) : null,
              ],
            ]}
          />
        </Section>
      ) : null}
      {retraction ? (
        <Section title="Retraction">
          <Fields
            rows={[
              ["Keys", `${retraction.received} named, ${retraction.unknown} unknown`],
              [
                "Taken back",
                `${count(retraction.retracted, "record")}, ${retraction.detached} of them left an identity`,
              ],
              ["Left for review", leftForReview(retraction.possible_splits)],
              [
                "Committed",
                retraction.committed ? (
                  <Ref
                    kind="log"
                    id={retraction.committed.seq}
                    label={`log position ${retraction.committed.seq}`}
                  />
                ) : null,
              ],
            ]}
          />
        </Section>
      ) : null}
      {extraction ? (
        <Section title="Extraction">
          <Fields
            rows={[
              ["Documents", `${extraction.documents} read`],
              [
                "Mentions",
                `${extraction.mentions} found: ${extraction.inserted} new, ${extraction.duplicates} duplicates, ${extraction.superseded} new versions`,
              ],
              [
                "Load run",
                extraction.load_run ? (
                  <Ref kind="run" id={extraction.load_run} label={`load run ${extraction.load_run}`} />
                ) : null,
              ],
            ]}
          />
        </Section>
      ) : null}
      {resolution ? (
        <>
          <Section title="Resolution">
            <Fields
              rows={[
                [
                  "Examined",
                  `${count(resolution.new_records, "record")}, ${resolution.reexamined} of them from earlier review items`,
                ],
                ["Candidates", count(resolution.candidates, "pair")],
                [
                  "Judgements",
                  `${resolution.judgements.match} match, ${resolution.judgements.no_match} no_match, ${resolution.judgements.unsure} unsure`,
                ],
                [
                  "Assigned",
                  `${count(resolution.records_assigned, "record")}, ${count(resolution.identities_created, "identity")} created`,
                ],
                [
                  "Unassigned",
                  `${count(resolution.records_unassigned, "record")} in ${count(resolution.review.length, "review item")}`,
                ],
                [
                  "Reconcile run",
                  <Ref kind="run" id={resolution.reconcile_run} label={`run ${resolution.reconcile_run}`} />,
                ],
                [
                  resolution.committed || !resolution.draft ? "Committed" : "Draft",
                  resolution.committed ? (
                    <Ref kind="log" id={resolution.committed.seq} label={`log position ${resolution.committed.seq}`} />
                  ) : resolution.draft ? (
                    <Ref kind="draft" id={resolution.draft} />
                  ) : (
                    "nothing to commit"
                  ),
                ],
              ]}
            />
          </Section>
          {resolution.review.length > 0 ? (
            <Section title="Review items">
              <Lines>
                {resolution.review.map((item, index) => (
                  <li key={index}>
                    {item.kind === "possible_merge"
                      ? `${count(item.records.length, "record")} matched ${count(item.identities.length, "identity")}: `
                      : `conflicting evidence about ${count(item.records.length, "record")}: `}
                    {item.records.map((record, index) => (
                      <span key={record}>
                        {index > 0 ? ", " : ""}
                        <Ref kind="record" id={record} />
                      </span>
                    ))}
                  </li>
                ))}
              </Lines>
            </Section>
          ) : null}
          {resolution.suggestions && resolution.suggestions.merges.length + resolution.suggestions.splits.length > 0 ? (
            <Section title="Suggestions" about="Not applied.">
              <Lines>
                {resolution.suggestions.merges.map((merge) => (
                  <li key={merge.identities.join()}>
                    merge <Identity id={merge.identities[0]} labels={labels} /> and{" "}
                    <Identity id={merge.identities[1]} labels={labels} />: {merge.matches} match, {merge.no_matches}{" "}
                    no_match between their members
                  </li>
                ))}
                {resolution.suggestions.splits.map((split) => (
                  <li key={split.identity}>
                    split <Identity id={split.identity} labels={labels} />: {split.no_matches} no_match, {split.matches}{" "}
                    match between its members
                  </li>
                ))}
              </Lines>
            </Section>
          ) : null}
        </>
      ) : null}
    </>
  );
}
