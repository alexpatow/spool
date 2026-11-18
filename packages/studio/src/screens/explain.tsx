import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done } from "../act.tsx";
import type { Comparison, ConstraintKind, Explained, Explanation, Link, RuleReading, RuleResult } from "../api.ts";
import {
  Code,
  Fields,
  Placed,
  Labelled,
  Lines,
  Load,
  message,
  Notice,
  Page,
  Panel,
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
import { findRecord } from "../find.ts";
import { recordName, plain } from "../format.ts";
import { useLabels, useLoad, useStudio } from "../hooks.ts";
import { href, navigate } from "../router.ts";
import type { Route } from "../router.ts";
import { Who } from "../who.tsx";

export function Explain({ route }: { route: Route }) {
  const { a, b } = route.query;
  return (
    <Page title="Explain">
      <Ask a={a} b={b} />
      {a && b ? <Answer a={a} b={b} /> : null}
    </Page>
  );
}

/** Takes the two records, by ID or as `source:key`, and goes to their explanation. */
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
            setProblem("Enter two records.");
            return;
          }
          setBusy(true);
          setProblem(null);
          Promise.all(references.map((reference) => findRecord(client, reference))).then(
            (found) => {
              setBusy(false);
              const missing = references.find((_, index) => found[index] === null);
              if (missing !== undefined) setProblem(`No record found for ${missing}.`);
              else navigate(href(["explain"], { a: found[0]!.id, b: found[1]!.id }));
            },
            (error: unknown) => {
              setBusy(false);
              setProblem(message(error));
            },
          );
        }}
      >
        <Labelled label="Left record">
          <Input
            name="a"
            defaultValue={a ?? ""}
            placeholder="ID or source:key"
            spellCheck={false}
            autoComplete="off"
            style={styles.box}
          />
        </Labelled>
        <Labelled label="Right record">
          <Input
            name="b"
            defaultValue={b ?? ""}
            placeholder="ID or source:key"
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
  same_identity: "Same identity",
  different_identities: "Different identities",
  unassigned: "Not both assigned",
  different_types: "Different entity types",
};

const OUTCOME_TONES: Record<string, Tone> = {
  same_identity: "good",
  unassigned: "waits",
};

function ExplanationView({ explanation }: { explanation: Explanation }) {
  const { left, right } = explanation;
  const names = new Map([left, right].map(({ record }) => [record.id, recordName(record)]));
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
        <Section title="Comparisons">
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
                title: "Block rules",
                cell: (comparison) =>
                  comparison.blocked_by.length === 0
                    ? "none"
                    : comparison.blocked_by.map((rule, index) => (
                        <span key={rule}>
                          {index > 0 ? ", " : ""}
                          <Code>{rule}</Code>
                        </span>
                      )),
              },
              {
                title: "Judgements",
                wrap: true,
                cell: (comparison) =>
                  comparison.judgements.length === 0
                    ? "none"
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
            empty="No comparisons."
          />
        </Section>
      ) : null}
      <Constrain explanation={explanation} />
      {explanation.review_items.length > 0 ? (
        <Section title="Review items">
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
        <Section title="Path">
          <Chain path={explanation.path} names={names} />
        </Section>
      ) : null}
    </>
  );
}

const KINDS: Record<ConstraintKind, string> = {
  must_link: "always the same identity",
  cannot_link: "never the same identity",
};

/**
 * Records what a person knows about the two, or takes it back. A pair has
 * one constraint at a time, and one that where they are now would break is
 * not offered.
 */
function Constrain({ explanation }: { explanation: Explanation }) {
  const [done, setDone] = useState<ReactNode>(null);
  // Two records of different entity types are never compared, so nothing is said of them.
  if (explanation.outcome === "different_types") return null;
  const a = explanation.left.record.id;
  const b = explanation.right.record.id;
  const inForce = explanation.constraints.filter((record) => record.to === null || record.to === undefined);
  const kinds: ConstraintKind[] =
    explanation.outcome === "same_identity"
      ? ["must_link"]
      : explanation.outcome === "different_identities"
        ? ["cannot_link"]
        : ["must_link", "cannot_link"];
  return (
    <Section title="Constraints" about="A constraint overrides what runs decide for this pair.">
      <Done>{done}</Done>
      {explanation.constraints.length > 0 ? (
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
                ", active"
              )}
            </li>
          ))}
        </Lines>
      ) : null}
      <Row>
        {inForce.map((record) => (
          <Act
            key={record.kind}
            label={`Remove ${record.kind}`}
            why="Reason"
            act={async (client, { reason }) => {
              const committed = await client.commit({
                reason,
                ops: [{ op: "unconstrain", kind: record.kind, a: record.a, b: record.b }],
              });
              return (
                <>
                  Removed {record.kind} at <At committed={committed} />.
                </>
              );
            }}
            done={setDone}
          />
        ))}
        {inForce.length === 0 ? (
          <Act
            label="Add constraint"
            why="Reason"
            act={async (client, { reason, entered }) => {
              const kind = String(entered.get("kind")) as ConstraintKind;
              const committed = await client.commit({ reason, ops: [{ op: "constrain", kind, a, b }] });
              return (
                <>
                  Added {kind} at <At committed={committed} />.
                </>
              );
            }}
            done={setDone}
          >
            <Labelled label="Kind">
              <Select
                name="kind"
                defaultValue={kinds[0]}
                choices={kinds.map((kind) => ({ value: kind, label: `${kind}: ${KINDS[kind]}` }))}
              />
            </Labelled>
          </Act>
        ) : null}
      </Row>
    </Section>
  );
}

function Side({ title, letter, explained }: { title: string; letter: string; explained: Explained }) {
  const { record, placed } = explained;
  const labels = useLabels([record.identity]);
  return (
    <div data-part="side" {...stylex.props(styles.side)}>
      <div>
        <div {...stylex.props(styles.sideKind)}>
          {title} ({letter} in a rule)
        </div>
        <h2 {...stylex.props(styles.sideName)}>{record.label ?? recordName(record)}</h2>
      </div>
      <Fields
        rows={[
          ["Record", <Ref kind="record" id={record.id} label={recordName(record)} />],
          ["Source", <Source name={record.source} />],
          ["ID", <Ref kind="record" id={record.id} />],
          ["Entity type", record.entity_type],
          [
            "Identity",
            <Placed record={record} labels={labels} />,
          ],
          [
            "Assigned",
            placed ? (
              <>
                at <Ref kind="log" id={placed.seq} label={`log position ${placed.seq}`} /> by{" "}
                <Who actor={placed.actor} />
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
    <Section title="Features">
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
                    {same ? <CheckIcon aria-label="Same" aria-hidden={false} /> : null}
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
    if (row.group !== "block" && rules.failed) return <Word value="failed" label="error" />;
    return <Word value="unsure" label="unknown: a feature is null" />;
  };
  return (
    <Section title="Current rules">
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
          { title: "Result", cell: reading },
        ]}
        rows={rows}
        rowKey={(row) => `${row.group}/${row.name}`}
        empty="No rules."
      />
      {rules.failed ? <Notice tone="bad">The no_match and match rules failed: {rules.failed}</Notice> : null}
      <Text>
        {rules.candidate ? (
          rules.verdict ? (
            <>
              A run would judge this pair <Word value={rules.verdict} />.
            </>
          ) : (
            "A run would compare this pair and make no judgement."
          )
        ) : (
          "No block rule holds, so a run would not compare this pair."
        )}
      </Text>
    </Section>
  );
}

const LINKS: Record<Link["kind"], string> = {
  must_link: "must_link constraint",
  version: "later version",
  match: "match",
};

function Chain({ path, names }: { path: Link[]; names: Map<string, string> }) {
  return (
    <ol data-part="chain" {...stylex.props(styles.chain)}>
      {path.map((link, index) => (
        <li key={index} {...stylex.props(styles.step)}>
          <Ref kind="record" id={link.from} label={names.get(link.from)} />
          <span {...stylex.props(styles.joins)}>
            {LINKS[link.kind]}
            {link.kind === "match" && link.run ? (
              <>
                {" "}
                in <Ref kind="run" id={link.run} label={`run ${link.run}`} />
              </>
            ) : null}
          </span>
          <Ref kind="record" id={link.to} label={names.get(link.to)} />
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
