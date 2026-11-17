import type { RunRecord, RunKind, RunStatus } from "../api.ts";
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
  Quiet,
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

const KINDS: RunKind[] = ["ingest", "extract", "match", "reconcile"];
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
    <Page title="Runs" about="Each ingest, extraction, match and reconcile the engine made, newest first.">
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
                  title: "About",
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

function RunView({ run }: { run: RunRecord }) {
  const { ingest, extraction, resolution } = run;
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
        <Section title="What it stored">
          <Fields
            rows={[
              [
                "Rows",
                `${ingest.received} received: ${ingest.inserted} new, ${ingest.duplicates} known, ${ingest.superseded ?? 0} new versions of a known row`,
              ],
            ]}
          />
        </Section>
      ) : null}
      {extraction ? (
        <Section title="What it found">
          <Fields
            rows={[
              ["Documents", `${extraction.documents} read`],
              [
                "Mentions",
                `${extraction.mentions} found: ${extraction.inserted} new, ${extraction.duplicates} known, ${extraction.superseded} new versions of a known mention`,
              ],
              [
                "Stored by",
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
          <Section title="What it made of them">
            <Fields
              rows={[
                [
                  "Examined",
                  `${count(resolution.new_observations, "observation")}, ${resolution.reexamined} of them left for review before`,
                ],
                ["Candidates", count(resolution.candidates, "pair")],
                [
                  "Judgements",
                  `${resolution.judgements.match} match, ${resolution.judgements.no_match} no_match, ${resolution.judgements.unsure} unsure`,
                ],
                [
                  "Placed",
                  `${count(resolution.observations_assigned, "observation")}, ${count(resolution.identities_created, "identity")} created`,
                ],
                [
                  "Left out",
                  `${count(resolution.observations_unassigned, "observation")} in ${count(resolution.review.length, "review item")}`,
                ],
                [
                  "Reconciled by",
                  <Ref kind="run" id={resolution.reconcile_run} label={`run ${resolution.reconcile_run}`} />,
                ],
                [
                  resolution.committed || !resolution.draft ? "Committed" : "Draft",
                  resolution.committed ? (
                    <Ref kind="log" id={resolution.committed.seq} label={`log position ${resolution.committed.seq}`} />
                  ) : resolution.draft ? (
                    <>
                      <Ref kind="draft" id={resolution.draft} /> was left for review
                    </>
                  ) : (
                    "nothing to commit"
                  ),
                ],
              ]}
            />
          </Section>
          {resolution.review.length > 0 ? (
            <Section title="What it would not decide">
              <Lines>
                {resolution.review.map((item, index) => (
                  <li key={index}>
                    {item.kind === "possible_merge"
                      ? `${count(item.observations.length, "observation")} matched ${count(item.identities.length, "identity")}, which may be one: `
                      : `the evidence about ${count(item.observations.length, "observation")} contradicts itself: `}
                    {item.observations.map((observation, index) => (
                      <span key={observation}>
                        {index > 0 ? ", " : ""}
                        <Ref kind="observation" id={observation} />
                      </span>
                    ))}
                  </li>
                ))}
              </Lines>
            </Section>
          ) : null}
          {resolution.suggestions && resolution.suggestions.merges.length + resolution.suggestions.splits.length > 0 ? (
            <Section title="Where the strategy disagrees with the identities as they are">
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
              <Quiet>Nothing here was applied.</Quiet>
            </Section>
          ) : null}
        </>
      ) : null}
    </>
  );
}
