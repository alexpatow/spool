"use client";

// The last two scenes of the explainer: records resolved into identities, and
// an identity that keeps its ID through what comes after.

import * as stylex from "@stylexjs/stylex";

import { Mark } from "spool-ui/brand";
import { Part, parts, timing, vars } from "@/components/explainer/part";
import { mentions, qualifiedKey, rows } from "@/components/explainer/records";
import { HEIGHT, WIDTH } from "@/components/explainer/scene";
import { identities, records } from "@/lib/sample";
import { colors, display, linen, typography } from "spool-ui/tokens.stylex";

const acorn = identities[0];

/* ------------------------------------------------------------------ */
/* 3. Resolution                                                       */
/* ------------------------------------------------------------------ */

const LIST_X = 64;
const LIST_Y = 36;
const PITCH = 32;
const CHIP_W = 200;
const CHIP_H = 26;

/** The fourteen pairs the three runs of Getting started judged, in the order they ran. */
const pairs: { a: string; b: string; match: boolean }[] = [
  { a: "c-1", b: "c-2", match: true },
  { a: "c-1", b: "c-3", match: true },
  { a: "c-2", b: "c-3", match: true },
  { a: "c-5", b: "c-6", match: false },
  { a: "c-4", b: "c-7", match: true },
  { a: "c-1", b: "c-9", match: true },
  { a: "c-2", b: "c-9", match: true },
  { a: "c-3", b: "c-9", match: true },
  ...["c-1", "c-2", "c-3", "c-9"].map((key) => ({ a: key, b: mentions[0]?.key ?? "", match: true })),
  ...["c-4", "c-7"].map((key) => ({ a: key, b: mentions[1]?.key ?? "", match: true })),
];

/** Where each identity's box is once the records are placed: two columns. */
const COLUMN_W = 206;
const HEAD = 42;
const MEMBER = 30;
const GAP = 12;
const columns = [
  [0, 2, 4],
  [1, 3, 5],
];
const boxes = (() => {
  const out = new Map<number, { x: number; y: number; height: number }>();
  columns.forEach((column, c) => {
    let y = 30;
    for (const identity of column) {
      const members = records.filter((record) => record.identity === identity).length;
      const height = HEAD + members * MEMBER + 6;
      out.set(identity, { x: c * (COLUMN_W + 8), y, height });
      y += height + GAP;
    }
  });
  return out;
})();

export function Resolution() {
  const index = (key: string) => records.findIndex((record) => record.key === key);
  const centre = (at: number) => LIST_Y + at * PITCH + CHIP_H / 2;
  const matches = pairs.filter((pair) => pair.match).length;

  return (
    <>
      <Part x={LIST_X} y={0} until={3} style={s.label}>
        {records.length} records
      </Part>

      <svg width={WIDTH} height={HEIGHT} {...stylex.props(s.lines)}>
        {pairs.map((pair, order) => {
          const from = centre(index(pair.a));
          const to = centre(index(pair.b));
          const bend = Math.min(LIST_X - 6, 10 + Math.abs(to - from) * 0.16);
          return (
            <path
              key={`${pair.a}-${pair.b}`}
              d={`M${LIST_X} ${from}C${LIST_X - bend} ${from} ${LIST_X - bend} ${to} ${LIST_X} ${to}`}
              pathLength={1}
              {...stylex.props(parts.stroke, pair.match ? s.match : s.noMatch)}
              style={timing(
                pair.match ? { at: 1, until: 3, delay: 70 * order, duration: 700 } : { at: 2, until: 3, duration: 900 },
              )}
            />
          );
        })}
      </svg>

      <Part x={LIST_X + CHIP_W + 14} y={centre(4) - 4} at={2} until={3} from={{ x: -10 }} delay={500} style={s.verdict}>
        <span {...stylex.props(s.cross)}>&times;</span> no_match
        <span {...stylex.props(s.because)}>different_country</span>
      </Part>

      {[...boxes].map(([identity, box], order) => {
        const held = identities[identity];
        return (
          <Part
            key={identity}
            x={box.x}
            y={box.y}
            width={COLUMN_W}
            height={box.height}
            at={3}
            from={{ scale: 0.94 }}
            delay={250 + 60 * order}
            style={[parts.card, s.identity]}
          >
            <span {...stylex.props(s.identityName)}>
              <Mark width={14} height={14} {...stylex.props(s.mark)} />
              {held?.name}
              {held && "detail" in held ? <span {...stylex.props(s.detail)}>{held.detail}</span> : null}
            </span>
            <span {...stylex.props(s.identityId)}>{held?.id}</span>
          </Part>
        );
      })}

      {records.map((record, at) => {
        const box = boxes.get(record.identity);
        const place = records.filter((other) => other.identity === record.identity).indexOf(record);
        const x = (box?.x ?? 0) + 3;
        const y = (box?.y ?? 0) + HEAD + place * MEMBER;
        return (
          <Part
            key={qualifiedKey(record)}
            x={LIST_X}
            y={LIST_Y + at * PITCH}
            width={CHIP_W}
            height={CHIP_H}
            from={{ y: 10 }}
            delay={40 * at}
            move={{ x: x - LIST_X, y: y - (LIST_Y + at * PITCH), at: 3 }}
            lit={record.key === "c-5" || record.key === "c-6" ? { at: 2, until: 3 } : { at: 999 }}
            style={[s.record, parts.litBorder]}
          >
            <span {...stylex.props(s.recordName)}>{record.name}</span>
            <span {...stylex.props(s.recordTag)}>{record.country ?? "text"}</span>
          </Part>
        );
      })}

      <Part x={LIST_X} y={HEIGHT - 84} at={1} until={3} style={s.tally}>
        <span {...stylex.props(s.label)}>judgements</span>
        <span {...stylex.props(s.tallyMatch)}>{matches} match</span>
        <span
          {...stylex.props(parts.part, s.tallyNo)}
          style={{ ...timing({ at: 2 }), ...vars({ "--r": 0 }), position: "relative" }}
        >
          {pairs.length - matches} no_match
        </span>
      </Part>

      <Part x={0} y={HEIGHT - 84} at={3} delay={900} from={{ y: 8 }} style={s.tally}>
        <span {...stylex.props(s.label)}>identities</span>
        <span {...stylex.props(s.tallyMatch)}>
          {identities.length} companies, {records.length} records
        </span>
      </Part>
    </>
  );
}

/* ------------------------------------------------------------------ */
/* 4. Persistent identity                                              */
/* ------------------------------------------------------------------ */

const tooling = { key: "c-10", name: "Acorn Tooling", source: "crm" };
/** The ID the split gave Acorn Tooling, as the Identities page shows it. */
const toolingId = "01a1f0a8-2062-70aa-a42f-c8855877ed84";

const members = [
  ...rows.filter((record) => record.identity === 0 && record.key !== "c-9").map((record) => ({ record, at: 0 })),
  { record: rows.find((record) => record.key === "c-9"), at: 1 },
  { record: mentions[0], at: 2 },
  { record: tooling, at: 3 },
];

const log = [
  { actor: "engine", did: "crm-1.csv: create Acorn Tools, place 3 records" },
  { actor: "engine", did: "crm-2.csv: place crm:c-9" },
  { actor: "engine", did: `notes-1.txt: place ${mentions[0] ? qualifiedKey(mentions[0]) : ""}` },
  { actor: "engine", did: "crm-3.csv: place crm:c-10" },
  { actor: "user", did: "\u201cAcorn Tooling is another company\u201d: split" },
];

const ROW_Y = 112;
const ROW_PITCH = 30;
const SPLIT_Y = 318;
const LOG_Y = 420;

export function Persistent() {
  return (
    <>
      <Part x={0} y={0} width={WIDTH} height={ROW_Y + 6 * ROW_PITCH + 4} style={[parts.card]} />
      <Part x={16} y={16} style={s.bigName}>
        <Mark width={20} height={20} {...stylex.props(s.mark)} />
        {acorn.name}
      </Part>
      <Part x={16} y={52} style={s.bigId}>
        {acorn.id}
      </Part>
      {[1, 2, 3, 4].map((step) => (
        <Part
          key={step}
          x={16}
          y={76}
          at={step}
          until={step === 4 ? 999 : step + 1}
          from={{ y: 6 }}
          delay={500}
          style={s.kept}
        >
          &#10003; same ID after {step + 1} changesets
        </Part>
      ))}
      <Part x={WIDTH - 90} y={20} style={s.label}>
        company
      </Part>

      <Part
        x={0}
        y={SPLIT_Y}
        width={WIDTH}
        height={HEAD + 40}
        at={4}
        from={{ y: 24, scale: 0.97 }}
        style={[parts.card, s.identity]}
      >
        <span {...stylex.props(s.identityName)}>
          <Mark width={14} height={14} {...stylex.props(s.mark)} />
          {tooling.name}
          <span {...stylex.props(s.detail)}>cannot_link with Acorn Tools</span>
        </span>
        <span {...stylex.props(s.identityId)}>{toolingId}</span>
      </Part>

      {members.map(({ record, at }, place) =>
        record ? (
          <Part
            key={record.key}
            x={12}
            y={ROW_Y + place * ROW_PITCH}
            width={WIDTH - 24}
            height={26}
            at={at}
            from={{ x: 36 }}
            delay={at === 0 ? 0 : 200}
            lit={{ at, until: at + 1 }}
            move={record === tooling ? { y: SPLIT_Y + HEAD + 4 - (ROW_Y + place * ROW_PITCH), at: 4 } : undefined}
            style={[s.member, parts.litWash]}
          >
            <span {...stylex.props(s.recordName)}>{record.name}</span>
            <span {...stylex.props(s.recordTag)}>
              {record.source}:{record.key}
            </span>
          </Part>
        ) : null,
      )}

      <Part x={0} y={LOG_Y} style={s.label}>
        changeset log
      </Part>
      {log.map((entry, position) => (
        <Part
          key={position}
          x={0}
          y={LOG_Y + 22 + position * 26}
          width={WIDTH}
          at={position}
          from={{ y: -10 }}
          delay={100}
          lit={{ at: position, until: position + 1 }}
          style={[s.entry, parts.litText]}
        >
          <span {...stylex.props(s.position)}>{position + 1}</span>
          <span {...stylex.props(s.actor)}>{entry.actor}</span>
          <span {...stylex.props(s.did)}>{entry.did}</span>
        </Part>
      ))}
    </>
  );
}

const s = stylex.create({
  label: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: 11,
    letterSpacing: "0.06em",
    lineHeight: "16px",
    textTransform: "uppercase",
    whiteSpace: "nowrap",
  },
  lines: {
    position: "absolute",
    left: 0,
    overflow: "visible",
    pointerEvents: "none",
    top: 0,
  },
  match: {
    stroke: linen.thread,
    strokeWidth: 1.25,
  },
  noMatch: {
    stroke: colors.destructiveText,
    strokeDasharray: "1",
    strokeWidth: 1.75,
  },
  verdict: {
    color: colors.destructiveText,
    display: "flex",
    flexDirection: "column",
    fontFamily: typography.fontFamilyMono,
    fontSize: 12,
    lineHeight: "16px",
    whiteSpace: "nowrap",
  },
  cross: {
    fontSize: 16,
  },
  because: {
    color: colors.mutedForeground,
    fontSize: 10.5,
  },
  record: {
    gap: 8,
    paddingInline: 10,
    alignItems: "center",
    backgroundColor: colors.card,
    borderColor: colors.border,
    borderStyle: "solid",
    borderWidth: 1,
    display: "flex",
  },
  recordName: {
    color: colors.foreground,
    flexGrow: 1,
    fontSize: 12.5,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  recordTag: {
    color: colors.mutedForeground,
    flexShrink: 0,
    fontFamily: typography.fontFamilyMono,
    fontSize: 10.5,
  },
  identity: {
    paddingBlock: 7,
    paddingInline: 10,
    backgroundColor: `color-mix(in oklab, ${linen.threadWash} 70%, ${colors.card})`,
    borderColor: `color-mix(in oklab, ${linen.thread} 45%, ${colors.border})`,
    display: "flex",
    flexDirection: "column",
    transformOrigin: "top left",
  },
  identityName: {
    gap: 6,
    alignItems: "center",
    color: colors.foreground,
    display: "flex",
    fontFamily: display.fontFamily,
    fontSize: 15,
    fontWeight: linen.displayWeight,
    lineHeight: "18px",
    whiteSpace: "nowrap",
  },
  detail: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamily,
    fontSize: 11,
  },
  identityId: {
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: 8.5,
    lineHeight: "14px",
    marginTop: 1,
    whiteSpace: "nowrap",
  },
  mark: {
    color: linen.thread,
    flexShrink: 0,
  },
  tally: {
    gap: 12,
    alignItems: "baseline",
    display: "flex",
    fontFamily: typography.fontFamilyMono,
    fontSize: 13,
    whiteSpace: "nowrap",
  },
  tallyMatch: {
    color: linen.thread,
  },
  tallyNo: {
    color: colors.destructiveText,
  },
  bigName: {
    gap: 10,
    alignItems: "center",
    color: colors.foreground,
    display: "flex",
    fontFamily: display.fontFamily,
    fontSize: 26,
    fontWeight: linen.displayWeight,
    lineHeight: "30px",
  },
  bigId: {
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: 12.5,
  },
  kept: {
    color: colors.mutedForeground,
    fontSize: 12,
    whiteSpace: "nowrap",
  },
  member: {
    gap: 8,
    paddingInline: 10,
    alignItems: "center",
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "flex",
  },
  entry: {
    gap: 10,
    alignItems: "baseline",
    display: "flex",
    fontSize: 12,
    lineHeight: "22px",
    whiteSpace: "nowrap",
  },
  position: {
    fontFamily: typography.fontFamilyMono,
    width: 14,
  },
  actor: {
    fontFamily: typography.fontFamilyMono,
    width: 48,
  },
  did: {
    overflow: "hidden",
    textOverflow: "ellipsis",
  },
});
