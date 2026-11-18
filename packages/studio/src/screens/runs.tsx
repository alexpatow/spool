import type { ReactNode } from "react";

import type { RunEntry, RunKind, RunStatus } from "../api.ts";
import {
  Code,
  Fields,
  Filters,
  Identity,
  Lines,
  Load,
  Notice,
  Onward,
  Page,
  Ref,
  Section,
  Table,
  When,
  Word,
} from "../components.tsx";
import { RunIcon } from "../icons.tsx";
import { count } from "../format.ts";
import { useLabels, useLoad } from "../hooks.ts";
import type { Route } from "../router.ts";

const KINDS: RunKind[] = ["ingest", "extract", "match", "reconcile", "retract"];
const STATUSES: RunStatus[] = ["running", "open", "drafted", "succeeded", "failed"];
const STEP = 100;

export function Runs({ route, types }: { route: Route; types: string[] }) {
  const { kind, status, type } = route.query;
  const limit = Number(route.query.limit) || STEP;
  const runs = useLoad((client) =>
    client.runs({
      kind: kind as RunKind | undefined,
      status: status as RunStatus | undefined,
      entity_type: type,
      limit,
    }),
  );
  return (
    <Page title="Runs">
      <Filters
        path={["runs"]}
        query={route.query}
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
              columns={[
                { title: "Run", cell: (run) => <Ref kind="run" id={run.run} label={`Run ${run.run}`} /> },
                { title: "Kind", cell: (run) => run.kind },
                { title: "Status", cell: (run) => <Word value={run.status} /> },
                {
                  title: "Details",
                  cell: (run) =>
                    [run.entity_type, run.strategy?.name, run.extractor?.name].filter((part) => part).join(", "),
                },
                { title: "Started", cell: (run) => <When at={run.started_at} /> },
              ]}
              rows={runs}
              rowKey={(run) => run.run}
              empty="No runs."
            />
            {runs.length >= limit && limit < 1000 ? (
              <Onward path={["runs"]} query={{ ...route.query, limit: Math.min(1000, limit + STEP) }}>
                Show more
              </Onward>
            ) : null}
          </>
        )}
      </Load>
    </Page>
  );
}

export function Run({ id }: { id: string }) {
  const run = useLoad((client) => client.run(Number(id)));
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
      <Onward path={["review"]}>review</Onward>
    </>
  );
}

function RunView({ run }: { run: RunEntry }) {
  const { ingest, extraction, resolution, retraction } = run;
  // The identities a strategy would merge or split are named by ID.
  const labels = useLabels([
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
          ["Started", <When at={run.started_at} />],
          ["Finished", run.finished_at ? <When at={run.finished_at} /> : null],
        ]}
      />
      {run.error ? (
        <Notice tone="bad" role="alert">
          {run.error}
        </Notice>
      ) : null}
      {ingest ? (
        <Section title="Ingest">
          <Fields
            rows={[
              [
                "Rows",
                `${ingest.received} received: ${ingest.inserted} new, ${ingest.duplicates} duplicates, ${ingest.superseded ?? 0} new versions`,
              ],
              [
                "Came back",
                ingest.restored
                  ? `${count(ingest.restored, "record")} the source had taken back, ${ingest.returned ?? 0} of them to the identity they left`
                  : null,
              ],
              ["Left for review", leftForReview(ingest.possible_splits)],
              [
                "Committed",
                ingest.committed ? (
                  <Ref kind="log" id={ingest.committed.seq} label={`log position ${ingest.committed.seq}`} />
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
                "Ingest run",
                extraction.ingest_run ? (
                  <Ref kind="run" id={extraction.ingest_run} label={`ingest run ${extraction.ingest_run}`} />
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
