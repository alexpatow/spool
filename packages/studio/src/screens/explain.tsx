import { useState } from "react";

import type { Comparison, Explained, Explanation, Link, RuleReading, RuleResult } from "../api.ts";
import { Fields, Load, Page, Ref, Section, Sentence, Table, When, Word, message } from "../components.tsx";
import { findObservation } from "../find.ts";
import { observationName, plain } from "../format.ts";
import { useLoad, useStudio } from "../hooks.ts";
import { href } from "../router.ts";
import type { Route } from "../router.ts";

export function Explain({ route }: { route: Route }) {
  const { a, b } = route.query;
  return (
    <Page
      title="Explain"
      about="Why two observations share an identity, or why they do not, from what the store holds."
    >
      <Ask a={a} b={b} />
      {a && b ? <Answer a={a} b={b} /> : null}
    </Page>
  );
}

/** Takes the two observations, by ID or as `source:key`, and goes to their explanation. */
function Ask({ a, b }: { a?: string; b?: string }) {
  const { client } = useStudio();
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="filters"
      onSubmit={(event) => {
        event.preventDefault();
        const entered = new FormData(event.currentTarget);
        const references = ["a", "b"].map((name) => String(entered.get(name) ?? "").trim());
        if (references.some((reference) => reference === "")) {
          setProblem("An explanation is about two observations.");
          return;
        }
        setBusy(true);
        setProblem(null);
        Promise.all(references.map((reference) => findObservation(client, reference))).then(
          (found) => {
            setBusy(false);
            const missing = references.find((_, index) => found[index] === null);
            if (missing !== undefined) setProblem(`No observation is named ${missing}.`);
            else window.location.hash = href(["explain"], { a: found[0]!.id, b: found[1]!.id });
          },
          (error: unknown) => {
            setBusy(false);
            setProblem(message(error));
          },
        );
      }}
    >
      <label>
        One observation
        <input
          name="a"
          defaultValue={a ?? ""}
          placeholder="an ID, or source:key"
          spellCheck={false}
          autoComplete="off"
          size={40}
        />
      </label>
      <label>
        The other
        <input
          name="b"
          defaultValue={b ?? ""}
          placeholder="an ID, or source:key"
          spellCheck={false}
          autoComplete="off"
          size={40}
        />
      </label>
      <button type="submit" disabled={busy}>
        Explain
      </button>
      {problem ? (
        <p className="notice failure" role="alert">
          {problem}
        </p>
      ) : null}
    </form>
  );
}

function Answer({ a, b }: { a: string; b: string }) {
  const explanation = useLoad((client) => client.explain(a, b));
  return <Load state={explanation}>{(explanation) => <ExplanationView explanation={explanation} />}</Load>;
}

const OUTCOMES: Record<string, string> = {
  same_identity: "The same identity",
  different_identities: "Different identities",
  unassigned: "Not both placed",
  different_types: "Different entity types",
};

function ExplanationView({ explanation }: { explanation: Explanation }) {
  const { left, right } = explanation;
  const names = new Map([left, right].map(({ observation }) => [observation.id, observationName(observation)]));
  return (
    <>
      <div className={`outcome ${explanation.outcome}`}>
        <Word value={explanation.outcome} label={OUTCOMES[explanation.outcome] ?? explanation.outcome} />
        {explanation.summary.map((sentence, index) => (
          <Sentence key={index} text={sentence} />
        ))}
      </div>
      <div className="sides">
        <Side title="Left" letter="l" explained={left} />
        <Side title="Right" letter="r" explained={right} />
      </div>
      <Features left={left} right={right} />
      {explanation.rules ? <Rules rules={explanation.rules} /> : null}
      {explanation.comparisons.length > 0 ? (
        <Section title="Runs that compared them">
          <Table
            columns={[
              { title: "Run", cell: (comparison: Comparison) => <Ref kind="run" id={comparison.run} /> },
              { title: "Started", cell: (comparison) => <When at={comparison.started_at} /> },
              {
                title: "Strategy",
                cell: (comparison) => (comparison.strategy ? <code>{comparison.strategy.name}</code> : "-"),
              },
              {
                title: "Status",
                cell: (comparison) => (
                  <>
                    <Word value={comparison.status} />
                    {comparison.draft ? (
                      <>
                        {" "}
                        draft <Word value={comparison.draft} />
                      </>
                    ) : null}
                  </>
                ),
              },
              {
                title: "Found by",
                cell: (comparison) =>
                  comparison.blocked_by.length === 0
                    ? "no block rule"
                    : comparison.blocked_by.map((rule, index) => (
                        <span key={rule}>
                          {index > 0 ? ", " : ""}
                          <code>{rule}</code>
                        </span>
                      )),
              },
              {
                title: "Judged",
                cell: (comparison) =>
                  comparison.judgements.length === 0
                    ? "no judgement"
                    : comparison.judgements.map((judgement, index) => (
                        <span key={index}>
                          {index > 0 ? " " : ""}
                          <Word value={judgement.verdict} />
                          {judgement.score !== null && judgement.score !== undefined ? ` (${judgement.score})` : ""}
                          {judgement.detail !== null && judgement.detail !== undefined ? (
                            <code className="detail"> {plain(judgement.detail)}</code>
                          ) : null}
                        </span>
                      )),
              },
            ]}
            rows={explanation.comparisons}
            rowKey={(comparison) => comparison.run}
            empty="No run compared them."
          />
        </Section>
      ) : null}
      {explanation.constraints.length > 0 ? (
        <Section title="Constraints between them">
          <ul className="plain">
            {explanation.constraints.map((record) => (
              <li key={`${record.kind}-${record.from}`}>
                <Word value={record.kind} /> from{" "}
                <Ref kind="log" id={record.from} label={`log position ${record.from}`} />
                {record.to !== null && record.to !== undefined ? (
                  <>
                    {" "}
                    until <Ref kind="log" id={record.to} label={`position ${record.to}`} />
                  </>
                ) : (
                  ", in force"
                )}
              </li>
            ))}
          </ul>
        </Section>
      ) : null}
      {explanation.review_items.length > 0 ? (
        <Section title="Review items that name them">
          <ul className="plain">
            {explanation.review_items.map((item) => (
              <li key={item.id}>
                <Ref kind="review" id={item.id} label={`item ${item.id}`} /> <Word value={item.kind} />{" "}
                <Word value={item.status} />
              </li>
            ))}
          </ul>
        </Section>
      ) : null}
      {explanation.path.length > 1 ? (
        <Section title="What joins them">
          <Chain path={explanation.path} names={names} />
        </Section>
      ) : null}
    </>
  );
}

function Side({ title, letter, explained }: { title: string; letter: string; explained: Explained }) {
  const { observation, placed } = explained;
  return (
    <div className="side">
      <h2>
        {title} <span className="quiet">({letter} in a rule)</span>
      </h2>
      <Fields
        rows={[
          ["Observation", <Ref kind="observation" id={observation.id} label={observationName(observation)} />],
          ["ID", <Ref kind="observation" id={observation.id} />],
          ["Entity type", observation.entity_type],
          [
            "Identity",
            observation.identity ? (
              <Ref kind="identity" id={observation.identity} />
            ) : (
              <Word value="unassigned" label="none" />
            ),
          ],
          [
            "Placed",
            placed ? (
              <>
                at <Ref kind="log" id={placed.seq} label={`log position ${placed.seq}`} /> by{" "}
                <code>{placed.actor}</code>
                {placed.run ? (
                  <>
                    , drafted by <Ref kind="run" id={placed.run} label={`reconcile run ${placed.run}`} />
                  </>
                ) : null}
                {placed.reason ? <span className="reason"> “{placed.reason}”</span> : null}
              </>
            ) : null,
          ],
        ]}
      />
    </div>
  );
}

function Features({ left, right }: { left: Explained; right: Explained }) {
  const names = [...new Set([left, right].flatMap((side) => Object.keys(side.features ?? {})))];
  if (names.length === 0) return null;
  const value = (side: Explained, name: string) => side.features?.[name];
  return (
    <Section title="Features">
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Feature</th>
              <th>Left</th>
              <th>Right</th>
            </tr>
          </thead>
          <tbody>
            {names.map((name) => {
              const l = value(left, name);
              const r = value(right, name);
              const same = l !== null && l !== undefined && JSON.stringify(l) === JSON.stringify(r);
              return (
                <tr key={name} className={same ? "same" : undefined}>
                  <td>
                    <code>{name}</code>
                  </td>
                  <td>{plain(l)}</td>
                  <td>{plain(r)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <p className="quiet">A row is marked where the two have the same value.</p>
    </Section>
  );
}

function Rules({ rules }: { rules: RuleReading }) {
  const groups: Array<[string, RuleResult[]]> = [
    ["block", rules.block],
    ["no_match", rules.no_match],
    ["match", rules.match],
  ];
  const rows = groups.flatMap(([group, results]) => results.map((result) => ({ group, ...result })));
  const reading = (row: (typeof rows)[number]) => {
    if (row.holds === true) return <Word value="holds" />;
    if (row.holds === false) return <Word value="does_not_hold" label="does not hold" />;
    if (row.group !== "block" && rules.failed) return <Word value="failed" label="cannot be applied to these values" />;
    return <Word value="unsure" label="cannot be decided: a feature it reads is null" />;
  };
  return (
    <Section title="Rules as they are now">
      <Table
        columns={[
          { title: "Kind", cell: (row) => <code>{row.group}</code> },
          { title: "Rule", cell: (row) => <code>{row.name}</code> },
          { title: "Predicate", cell: (row) => <code className="sql">{row.predicate.trim()}</code> },
          { title: "On this pair", cell: reading },
        ]}
        rows={rows}
        rowKey={(row) => `${row.group}/${row.name}`}
        empty="The entity type has no rules."
      />
      {rules.failed ? (
        <p className="notice failure">Applying the no_match and match rules failed: {rules.failed}</p>
      ) : null}
      <p>
        {rules.candidate ? (
          rules.verdict ? (
            <>
              A run of the rules would compare the two and judge <Word value={rules.verdict} />.
            </>
          ) : (
            "A run of the rules would compare the two and make no judgement."
          )
        ) : (
          "No block rule holds, so a run of the rules would not compare the two."
        )}
      </p>
    </Section>
  );
}

const LINKS: Record<Link["kind"], string> = {
  must_link: "must_link constraint",
  version: "a later version of the same row",
  match: "match",
};

function Chain({ path, names }: { path: Link[]; names: Map<string, string> }) {
  return (
    <ol className="chain">
      {path.map((link, index) => (
        <li key={index}>
          <Ref kind="observation" id={link.from} label={names.get(link.from)} />
          <span className="link">
            {LINKS[link.kind]}
            {link.kind === "match" && link.run ? (
              <>
                {" "}
                in <Ref kind="run" id={link.run} label={`run ${link.run}`} />
              </>
            ) : null}
          </span>
          <Ref kind="observation" id={link.to} label={names.get(link.to)} />
        </li>
      ))}
    </ol>
  );
}
