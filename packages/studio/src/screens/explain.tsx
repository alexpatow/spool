import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done } from "../act.tsx";
import type { Comparison, ConstraintKind, Explained, Explanation, Link, RuleReading, RuleResult } from "../api.ts";
import {
  Code,
  Fields,
  Identity,
  Labelled,
  Lines,
  Load,
  message,
  Notice,
  Page,
  Panel,
  Quiet,
  Reason,
  Ref,
  Row,
  Section,
  Select,
  Sentence,
  Source,
  Table,
  Text,
  When,
  Word,
} from "../components.tsx";
import type { Tone } from "../components.tsx";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import * as ShelfTable from "../components/ui/table";
import { CheckIcon } from "../icons.tsx";
import { colors, radius, spacing, typography } from "../styles/shelf/tokens.stylex";
import { tones } from "../styles/tones.stylex";
import { findObservation } from "../find.ts";
import { observationName, plain } from "../format.ts";
import { useLabels, useLoad, useStudio } from "../hooks.ts";
import { href } from "../router.ts";
import type { Route } from "../router.ts";

export function Explain({ route }: { route: Route }) {
  const { a, b } = route.query;
  return (
    <Page title="Explain" about="Why two records share an identity, or why they do not, from what the store holds.">
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
    <>
      <form
        {...stylex.props(styles.ask)}
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
        <Labelled label="One observation">
          <Input
            name="a"
            defaultValue={a ?? ""}
            placeholder="an ID, or source:key"
            spellCheck={false}
            autoComplete="off"
            style={styles.box}
          />
        </Labelled>
        <Labelled label="The other">
          <Input
            name="b"
            defaultValue={b ?? ""}
            placeholder="an ID, or source:key"
            spellCheck={false}
            autoComplete="off"
            style={styles.box}
          />
        </Labelled>
        <Button type="submit" variant="outline" disabled={busy}>
          Explain
        </Button>
      </form>
      {problem ? (
        <Notice tone="bad" role="alert">
          {problem}
        </Notice>
      ) : null}
    </>
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

const OUTCOME_TONES: Record<string, Tone> = {
  same_identity: "good",
  unassigned: "waits",
};

function ExplanationView({ explanation }: { explanation: Explanation }) {
  const { left, right } = explanation;
  const names = new Map([left, right].map(({ observation }) => [observation.id, observationName(observation)]));
  return (
    <>
      <div
        data-part="outcome"
        data-outcome={explanation.outcome}
        {...stylex.props(styles.outcome, outcomeTones[OUTCOME_TONES[explanation.outcome] ?? "plain"])}
      >
        <div>
          <Word value={explanation.outcome} label={OUTCOMES[explanation.outcome] ?? explanation.outcome} />
        </div>
        {explanation.summary.map((sentence, index) => (
          <Sentence key={index} text={sentence} />
        ))}
      </div>
      <div {...stylex.props(styles.sides)}>
        <Side title="Left" letter="l" explained={left} />
        <Side title="Right" letter="r" explained={right} />
      </div>
      <Features left={left} right={right} />
      {explanation.rules ? <Rules rules={explanation.rules} /> : null}
      {explanation.comparisons.length > 0 ? (
        <Section title="Runs that compared them">
          <Table
            columns={[
              {
                title: "Run",
                cell: (comparison: Comparison) => (
                  <Ref kind="run" id={comparison.run} label={`Run ${comparison.run}`} />
                ),
              },
              { title: "Started", cell: (comparison) => <When at={comparison.started_at} /> },
              {
                title: "Strategy",
                cell: (comparison) => (comparison.strategy ? <Code>{comparison.strategy.name}</Code> : "-"),
              },
              {
                title: "Status",
                cell: (comparison) => (
                  <Row>
                    <Word value={comparison.status} />
                    {comparison.draft ? (
                      <>
                        {" "}
                        draft <Word value={comparison.draft} />
                      </>
                    ) : null}
                  </Row>
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
                          <Code>{rule}</Code>
                        </span>
                      )),
              },
              {
                title: "Judged",
                wrap: true,
                cell: (comparison) =>
                  comparison.judgements.length === 0
                    ? "no judgement"
                    : comparison.judgements.map((judgement, index) => (
                        <span key={index}>
                          {index > 0 ? " " : ""}
                          <Word value={judgement.verdict} />
                          {judgement.score !== null && judgement.score !== undefined ? ` (${judgement.score})` : ""}
                          {judgement.detail !== null && judgement.detail !== undefined ? (
                            <>
                              {" "}
                              <Code>{plain(judgement.detail)}</Code>
                            </>
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
          <Lines>
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
          </Lines>
        </Section>
      ) : null}
      <Constrain explanation={explanation} />
      {explanation.review_items.length > 0 ? (
        <Section title="Review items that name them">
          <Lines>
            {explanation.review_items.map((item) => (
              <li key={item.id}>
                <Row>
                  <Ref kind="review" id={item.id} label={`item ${item.id}`} /> <Word value={item.kind} />{" "}
                  <Word value={item.status} />
                </Row>
              </li>
            ))}
          </Lines>
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

const KINDS: Record<ConstraintKind, string> = {
  must_link: "must share an identity",
  cannot_link: "must not share an identity",
};

/**
 * Records what a person knows about the two, or takes it back. A pair has
 * one constraint at a time, and one that where they are now would break is
 * not offered.
 */
function Constrain({ explanation }: { explanation: Explanation }) {
  const [done, setDone] = useState<ReactNode>(null);
  if (explanation.outcome === "different_types") return null;
  const a = explanation.left.observation.id;
  const b = explanation.right.observation.id;
  const inForce = explanation.constraints.filter((record) => record.to === null || record.to === undefined);
  const kinds: ConstraintKind[] =
    explanation.outcome === "same_identity"
      ? ["must_link"]
      : explanation.outcome === "different_identities"
        ? ["cannot_link"]
        : ["must_link", "cannot_link"];
  return (
    <Section
      title="Constrain them"
      about="A constraint holds whatever a run judges: no run joins or parts the two against it."
    >
      <Done>{done}</Done>
      <Row>
        {inForce.map((record) => (
          <Act
            key={record.kind}
            label={`Remove the ${record.kind} constraint`}
            about="Runs are free to join or part the two again."
            who
            why="Reason"
            act={async (client, { actor, reason }) => {
              const committed = await client.commit({
                actor,
                reason,
                ops: [{ op: "unconstrain", kind: record.kind, a: record.a, b: record.b }],
              });
              return (
                <>
                  The {record.kind} constraint was removed at <At committed={committed} />.
                </>
              );
            }}
            done={setDone}
          />
        ))}
        {inForce.length === 0 ? (
          <Act
            label="Record it"
            title="Record what you know about the two"
            about="It is kept as a constraint, which every run from now on keeps to."
            who
            why="Reason"
            act={async (client, { actor, reason, entered }) => {
              const kind = String(entered.get("kind")) as ConstraintKind;
              const committed = await client.commit({ actor, reason, ops: [{ op: "constrain", kind, a, b }] });
              return (
                <>
                  The {kind} constraint was recorded at <At committed={committed} />.
                </>
              );
            }}
            done={setDone}
          >
            <Labelled label="The two">
              <Select
                name="kind"
                defaultValue={kinds[0]}
                choices={kinds.map((kind) => ({ value: kind, label: `${KINDS[kind]} (${kind})` }))}
              />
            </Labelled>
          </Act>
        ) : null}
      </Row>
    </Section>
  );
}

function Side({ title, letter, explained }: { title: string; letter: string; explained: Explained }) {
  const { observation, placed } = explained;
  const labels = useLabels([observation.identity]);
  return (
    <div data-part="side" {...stylex.props(styles.side)}>
      <div>
        <div {...stylex.props(styles.sideKind)}>
          {title} ({letter} in a rule)
        </div>
        <h2 {...stylex.props(styles.sideName)}>{observation.label ?? observationName(observation)}</h2>
      </div>
      <Fields
        rows={[
          ["Observation", <Ref kind="observation" id={observation.id} label={observationName(observation)} />],
          ["Source", <Source name={observation.source} />],
          ["ID", <Ref kind="observation" id={observation.id} />],
          ["Entity type", observation.entity_type],
          [
            "Identity",
            observation.identity ? (
              <Identity id={observation.identity} labels={labels} />
            ) : (
              <Word value="unassigned" label="none" />
            ),
          ],
          [
            "Placed",
            placed ? (
              <>
                at <Ref kind="log" id={placed.seq} label={`log position ${placed.seq}`} /> by{" "}
                <Code>{placed.actor}</Code>
                {placed.run ? (
                  <>
                    , drafted by <Ref kind="run" id={placed.run} label={`reconcile run ${placed.run}`} />
                  </>
                ) : null}
                {placed.reason ? (
                  <>
                    {" "}
                    <Reason>{placed.reason}</Reason>
                  </>
                ) : null}
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
    <Section title="Features" about="What the rules read of each. A row is marked where the two have the same value.">
      <Panel flush>
        <ShelfTable.Root aria-label="Features">
          <ShelfTable.Header>
            <ShelfTable.Row style={styles.headRow}>
              <ShelfTable.Head style={styles.cell}>Feature</ShelfTable.Head>
              <ShelfTable.Head style={styles.cell}>Left</ShelfTable.Head>
              <ShelfTable.Head style={styles.cell}>Right</ShelfTable.Head>
              <ShelfTable.Head style={styles.cell} />
            </ShelfTable.Row>
          </ShelfTable.Header>
          <ShelfTable.Body>
            {names.map((name) => {
              const l = value(left, name);
              const r = value(right, name);
              const same = l !== null && l !== undefined && JSON.stringify(l) === JSON.stringify(r);
              return (
                <ShelfTable.Row key={name} data-same={same ? "" : undefined} style={[styles.row, same && styles.same]}>
                  <ShelfTable.Cell style={styles.cell}>
                    <Code>{name}</Code>
                  </ShelfTable.Cell>
                  <ShelfTable.Cell style={[styles.cell, styles.wrap]}>{plain(l)}</ShelfTable.Cell>
                  <ShelfTable.Cell style={[styles.cell, styles.wrap]}>{plain(r)}</ShelfTable.Cell>
                  <ShelfTable.Cell style={[styles.cell, styles.sameMark]}>
                    {same ? <CheckIcon aria-label="The same" aria-hidden={false} /> : null}
                  </ShelfTable.Cell>
                </ShelfTable.Row>
              );
            })}
          </ShelfTable.Body>
        </ShelfTable.Root>
      </Panel>
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
          { title: "Kind", cell: (row) => <Code>{row.group}</Code> },
          { title: "Rule", cell: (row) => <Code>{row.name}</Code> },
          {
            title: "Predicate",
            cell: (row) => (
              <code data-part="sql" {...stylex.props(styles.sql)}>
                {row.predicate.trim()}
              </code>
            ),
            wrap: true,
          },
          { title: "On this pair", cell: reading },
        ]}
        rows={rows}
        rowKey={(row) => `${row.group}/${row.name}`}
        empty="The entity type has no rules."
      />
      {rules.failed ? <Notice tone="bad">Applying the no_match and match rules failed: {rules.failed}</Notice> : null}
      <Text>
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
      </Text>
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
    <ol data-part="chain" {...stylex.props(styles.chain)}>
      {path.map((link, index) => (
        <li key={index} {...stylex.props(styles.step)}>
          <Ref kind="observation" id={link.from} label={names.get(link.from)} />
          <span {...stylex.props(styles.joins)}>
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

const styles = stylex.create({
  ask: {
    gap: spacing["3"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
  },
  box: {
    width: "22rem",
    maxWidth: "100%",
  },
  outcome: {
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["2"],
    padding: spacing["4"],
    display: "flex",
    flexDirection: "column",
  },
  sides: {
    gap: spacing["4"],
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(20rem, 1fr))",
  },
  side: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["3"],
    padding: spacing["4"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  sideKind: {
    color: colors.mutedForeground,
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
  },
  sideName: {
    margin: 0,
    fontSize: typography.fontSizeBase,
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.01em",
    lineHeight: typography.lineHeightBase,
    overflowWrap: "anywhere",
  },
  headRow: {
    backgroundColor: colors.card,
  },
  row: {
    borderBottomWidth: { default: 1, ":last-child": 0 },
  },
  same: {
    backgroundColor: tones.goodSurface,
  },
  cell: {
    paddingBlock: spacing["2"],
    paddingInline: spacing["3"],
  },
  wrap: {
    overflowWrap: "anywhere",
    whiteSpace: "normal",
  },
  sameMark: {
    color: tones.good,
    textAlign: "end",
    width: "1.5rem",
  },
  sql: {
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
    whiteSpace: "pre-wrap",
  },
  chain: {
    margin: 0,
    padding: 0,
    gap: spacing["2"],
    display: "flex",
    flexDirection: "column",
    listStyleType: "none",
  },
  step: {
    gap: spacing["2"],
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
  },
  joins: {
    borderRadius: radius.full,
    paddingBlock: "0.0625rem",
    paddingInline: spacing["2"],
    backgroundColor: colors.muted,
    color: colors.mutedForeground,
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
  },
});

const outcomeTones = stylex.create({
  good: { borderColor: "transparent", backgroundColor: tones.goodSurface },
  bad: { borderColor: "transparent", backgroundColor: tones.badSurface },
  waits: { borderColor: "transparent", backgroundColor: tones.waitsSurface },
  plain: { borderColor: colors.border, backgroundColor: colors.card },
});
