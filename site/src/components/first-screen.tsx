"use client";

import * as stylex from "@stylexjs/stylex";
import { Fragment, useState } from "react";

import { Mark } from "spool-ui/brand";
import { identities, matchRuns, note, records } from "@/lib/sample";
import { breakpoints } from "@/styles/layout.stylex";
import { colors, display, linen, spacing, typography } from "spool-ui/tokens.stylex";

/** The heights the threads are drawn against, in pixels: a column's heading, and one row. */
const HEAD = 36;
const ROW = 28;
/** Room under the last record, which the note beside it needs. */
const FOOT = 84;
/** The identity that is lit when nothing else is chosen, and on a page without JavaScript. */
const DEFAULT = 0;
const LATEST_RUN = matchRuns.length - 1;

const found = records.filter((record) => record.source === "notes");

/** The note, cut at the places the extractor found, so each can be marked. */
function marked() {
  const parts: { text: string; identity?: number }[] = [];
  let at = 0;
  for (const record of found) {
    const start = note.text.indexOf(record.name, at);
    parts.push({ text: note.text.slice(at, start) }, { text: record.name, identity: record.identity });
    at = start + record.name.length;
  }
  parts.push({ text: note.text.slice(at) });
  return parts;
}

/** The identity of the row, record, name or identity that an event came from, if it came from one. */
function identityAt(target: EventTarget | null): number | undefined {
  if (!(target instanceof Element)) return undefined;
  const value = target.closest("[data-identity]")?.getAttribute("data-identity");
  return value == null ? undefined : Number(value);
}

/**
 * The front page's first screen: what came in, the records it became, and the
 * identities they were placed in, with a thread from each record to its
 * identity. Each match run shows the sample project at that point in Getting started.
 *
 * One identity is lit at a time, with its records, the rows and the name in
 * the note they came from, and the matches that placed them. Pointing at any
 * of those, or moving the focus to a record or an identity, lights its
 * identity. A tap does the same and stays.
 */
export function FirstScreen() {
  const [active, setActive] = useState(DEFAULT);
  const [run, setRun] = useState(LATEST_RUN);
  const visible = records.filter((record) => record.match <= run);
  const rows = visible.filter((record) => record.source === "crm");
  const hasNote = visible.length > rows.length;
  const shownIdentities = identities
    .map((identity, index) => ({
      identity,
      index,
      held: visible.filter((record) => record.identity === index),
    }))
    .filter(({ held }) => held.length > 0);
  const body = visible.length * ROW + FOOT;
  const height = HEAD + body;
  const card = body / shownIdentities.length;
  const choose = (target: EventTarget | null) => {
    const identity = identityAt(target);
    if (identity !== undefined) setActive(identity);
  };
  const showRun = (index: number) => {
    setRun(index);
    setActive(DEFAULT);
  };
  // The lit threads are drawn last, so they lie over the others.
  const strands = visible
    .map((record, index) => ({ record, index }))
    .sort((a, b) => Number(a.record.identity === active) - Number(b.record.identity === active));

  return (
    <figure
      {...stylex.props(styles.figure)}
      aria-label="The sample project: rows and a note, the records they became, and their identities"
      onPointerOver={(event) => {
        if (event.pointerType === "mouse") choose(event.target);
      }}
      onPointerLeave={(event) => {
        if (event.pointerType === "mouse") setActive(DEFAULT);
      }}
      onFocus={(event) => choose(event.target)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setActive(DEFAULT);
      }}
      onClick={(event) => choose(event.target)}
    >
      <div {...stylex.props(styles.log)}>
        <span {...stylex.props(styles.label)}>Match runs</span>
        <ol {...stylex.props(styles.ruler)}>
          {matchRuns.map((entry, index) => {
            const selected = run === index;
            return (
              <li key={entry.after} {...stylex.props(styles.run)}>
                <button
                  type="button"
                  aria-pressed={selected}
                  onClick={() => showRun(index)}
                  {...stylex.props(styles.tick, selected && styles.tickSelected)}
                >
                  <span {...stylex.props(styles.position, selected && styles.thread)}>{entry.label}</span>
                  <span {...stylex.props(styles.did)}>{entry.did}</span>
                </button>
              </li>
            );
          })}
        </ol>
      </div>

      <div {...stylex.props(styles.panel)}>
        <div {...stylex.props(styles.column, styles.sources)}>
          <div {...stylex.props(styles.head)}>
            <span {...stylex.props(styles.label)}>Rows</span>
            <span {...stylex.props(styles.aside)}>
              {run === 0 ? "samples/crm-1.csv" : "samples/crm-1.csv, crm-2.csv"}
            </span>
          </div>
          {rows.map((record) => (
            <div
              key={record.key}
              data-identity={record.identity}
              {...stylex.props(styles.line, record.identity === active && styles.lineLit)}
            >
              {record.key},{record.name},{record.country}
            </div>
          ))}
          {hasNote && (
            <>
              <div {...stylex.props(styles.head, styles.noteHead)}>
                <span {...stylex.props(styles.label)}>Text</span>
                <span {...stylex.props(styles.aside)}>{note.file}</span>
              </div>
              <p {...stylex.props(styles.note)}>
                {marked().map((part, index) =>
                  part.identity === undefined ? (
                    <Fragment key={index}>{part.text}</Fragment>
                  ) : (
                    <mark
                      key={index}
                      data-identity={part.identity}
                      {...stylex.props(styles.mention, part.identity === active && styles.mentionLit)}
                    >
                      {part.text}
                    </mark>
                  ),
                )}
              </p>
            </>
          )}
        </div>

        <div {...stylex.props(styles.column)}>
          <div {...stylex.props(styles.head)}>
            <span {...stylex.props(styles.label)}>Records</span>
            <span {...stylex.props(styles.aside)}>kept as they arrived</span>
            <span {...stylex.props(styles.count)}>{visible.length}</span>
          </div>
          <table {...stylex.props(styles.table)}>
            {/* The header row is read out and not seen. It also sets the widths of the columns. */}
            <thead>
              <tr>
                <th {...stylex.props(styles.heading, styles.key)}>
                  <span {...stylex.props(styles.hidden)}>Source and key</span>
                </th>
                <th {...stylex.props(styles.heading)}>
                  <span {...stylex.props(styles.hidden)}>Name</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {visible.map((record) => {
                const lit = record.identity === active;
                return (
                  <tr
                    key={`${record.source}:${record.key}`}
                    tabIndex={0}
                    data-identity={record.identity}
                    {...stylex.props(styles.row, lit && styles.rowLit)}
                  >
                    <td {...stylex.props(styles.cell, styles.key, lit && styles.thread)}>
                      {record.source}:{record.key}
                    </td>
                    <td {...stylex.props(styles.cell, styles.name, lit && styles.nameLit)}>{record.name}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {hasNote && (
            <p {...stylex.props(styles.foot)}>
              {rows.length} from rows, {found.length} from the note. A record from text is keyed by where it was found.
            </p>
          )}
        </div>

        <svg
          {...stylex.props(styles.threads)}
          viewBox={`0 0 100 ${height}`}
          height={height}
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          {strands.map(({ record, index }) => {
            const from = HEAD + index * ROW + ROW / 2;
            const position = shownIdentities.findIndex((identity) => identity.index === record.identity);
            const to = HEAD + position * card + card / 2;
            return (
              <path
                key={index}
                d={`M0 ${from}C50 ${from} 50 ${to} 100 ${to}`}
                fill="none"
                vectorEffect="non-scaling-stroke"
                {...stylex.props(styles.strand, record.identity === active && styles.strandLit)}
              />
            );
          })}
        </svg>

        <div {...stylex.props(styles.column, styles.identities)}>
          <div {...stylex.props(styles.head)}>
            <span {...stylex.props(styles.label)}>Identities</span>
            <span {...stylex.props(styles.aside)}>persistent IDs</span>
            <span {...stylex.props(styles.count)}>{shownIdentities.length}</span>
          </div>
          {shownIdentities.map(({ identity, index, held }) => {
            const lit = index === active;
            const sources = new Set(held.map((record) => record.source)).size;
            return (
              <button
                key={identity.id}
                type="button"
                data-identity={index}
                aria-pressed={lit}
                {...stylex.props(styles.identity, lit && styles.identityLit)}
                style={{ height: card }}
              >
                <span {...stylex.props(styles.identityTop)}>
                  <Mark width={16} height={16} {...stylex.props(styles.mark, lit && styles.thread)} />
                  <span {...stylex.props(styles.identityName, lit && styles.nameLit)}>{identity.name}</span>
                  {"detail" in identity ? <span {...stylex.props(styles.detail)}>{identity.detail}</span> : null}
                  <span {...stylex.props(styles.members)}>
                    {held.length} {held.length === 1 ? "record" : "records"} · {sources}{" "}
                    {sources === 1 ? "source" : "sources"}
                  </span>
                </span>
                <span {...stylex.props(styles.id, lit && styles.thread)}>{identity.id}</span>
              </button>
            );
          })}
        </div>
      </div>
    </figure>
  );
}

const hairline = { borderColor: colors.border, borderStyle: "solid", borderWidth: 0 } as const;

/** Colour only, and nothing for a reader who asked for less motion. */
const quietly = {
  transitionDuration: {
    default: "140ms",
    "@media (prefers-reduced-motion: reduce)": "0s",
  },
  transitionProperty: "color, background-color, border-color, stroke",
  transitionTimingFunction: "ease-out",
} as const;

const ring = {
  outline: {
    default: "none",
    ":focus-visible": `2px solid ${colors.foreground}`,
  },
  outlineOffset: -2,
} as const;

const styles = stylex.create({
  figure: {
    display: { default: "block", [breakpoints.narrow]: "none" },
    boxSizing: "border-box",
    ...hairline,
    margin: 0,
    borderWidth: 1,
    backgroundColor: colors.card,
    cursor: "default",
    textAlign: "start",
  },
  panel: {
    display: "grid",
    gridTemplateColumns: "minmax(0, 1.05fr) minmax(0, 1.45fr) 5rem minmax(0, 1.2fr)",
  },
  column: {
    minWidth: 0,
  },
  sources: {
    ...hairline,
    borderInlineEndWidth: 1,
    backgroundColor: colors.background,
  },
  identities: {
    ...hairline,
    borderInlineStartWidth: 1,
  },
  head: {
    ...hairline,
    gap: spacing["2"],
    paddingInline: spacing["4"],
    alignItems: "center",
    borderBottomWidth: 1,
    boxSizing: "border-box",
    display: "flex",
    height: HEAD,
  },
  noteHead: {
    borderTopWidth: 1,
    height: ROW,
  },
  label: {
    color: colors.foreground,
    fontSize: "0.6875rem",
    fontWeight: typography.fontWeightMedium,
    letterSpacing: "0.08em",
    textTransform: "uppercase",
  },
  aside: {
    color: colors.mutedForeground,
    fontSize: "0.75rem",
  },
  count: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.75rem",
    marginInlineStart: "auto",
  },
  foot: {
    margin: 0,
    paddingInline: spacing["4"],
    paddingTop: spacing["2"],
    color: colors.mutedForeground,
    fontSize: "0.75rem",
    lineHeight: "1.125rem",
  },
  line: {
    ...quietly,
    paddingInline: spacing["4"],
    boxSizing: "border-box",
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.75rem",
    lineHeight: `${ROW}px`,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
    height: ROW,
  },
  lineLit: {
    backgroundColor: linen.threadWash,
    color: colors.foreground,
  },
  note: {
    margin: 0,
    paddingBlock: spacing["2"],
    paddingInline: spacing["4"],
    color: colors.foreground,
    fontSize: "0.8125rem",
    lineHeight: "1.25rem",
  },
  mention: {
    ...quietly,
    backgroundColor: "transparent",
    color: "inherit",
    textDecorationColor: linen.lineStrong,
    textDecorationLine: "underline",
    textDecorationThickness: 1,
    textUnderlineOffset: 4,
  },
  mentionLit: {
    backgroundColor: linen.threadWash,
    color: linen.thread,
    textDecorationColor: linen.thread,
  },
  table: {
    borderCollapse: "collapse",
    tableLayout: "fixed",
    width: "100%",
  },
  heading: {
    padding: 0,
    paddingInlineStart: spacing["4"],
    height: 0,
    lineHeight: 0,
  },
  hidden: {
    position: "absolute",
    clip: "rect(0 0 0 0)",
    overflow: "hidden",
    height: 1,
    width: 1,
  },
  row: {
    ...quietly,
    ...ring,
    height: ROW,
  },
  rowLit: {
    backgroundColor: linen.threadWash,
  },
  cell: {
    ...quietly,
    paddingBlock: 0,
    paddingInlineEnd: 0,
    paddingInlineStart: spacing["4"],
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.71875rem",
    lineHeight: `${ROW}px`,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  key: {
    color: colors.mutedForeground,
    width: "12.5rem",
  },
  name: {
    lineHeight: `${ROW}px`,
    color: colors.mutedForeground,
    fontFamily: typography.fontFamily,
    fontSize: "0.8125rem",
  },
  nameLit: {
    color: colors.foreground,
  },
  thread: {
    color: linen.thread,
  },
  threads: {
    display: "block",
    pointerEvents: "none",
    width: "100%",
  },
  strand: {
    ...quietly,
    stroke: linen.lineStrong,
    strokeWidth: 1,
  },
  strandLit: {
    stroke: linen.thread,
    strokeWidth: 1.25,
  },
  identity: {
    ...hairline,
    ...quietly,
    ...ring,
    gap: 2,
    margin: 0,
    paddingBlock: 0,
    paddingInline: spacing["4"],
    appearance: "none",
    backgroundColor: "transparent",
    borderBottomWidth: { default: 1, ":last-child": 0 },
    borderRadius: 0,
    boxSizing: "border-box",
    color: "inherit",
    cursor: "default",
    display: "flex",
    flexDirection: "column",
    fontFamily: typography.fontFamily,
    fontSize: "inherit",
    justifyContent: "center",
    textAlign: "start",
    width: "100%",
  },
  identityLit: {
    backgroundColor: linen.threadWash,
  },
  identityTop: {
    gap: spacing["2"],
    alignItems: "baseline",
    display: "flex",
    minWidth: 0,
    width: "100%",
  },
  mark: {
    ...quietly,
    alignSelf: "center",
    color: colors.mutedForeground,
    flexShrink: 0,
  },
  identityName: {
    ...quietly,
    color: colors.mutedForeground,
    fontFamily: display.fontFamily,
    fontSize: "1.0625rem",
    fontVariationSettings: '"opsz" 36',
    fontWeight: linen.displayWeight,
    lineHeight: "1.5rem",
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  detail: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamily,
    fontSize: "0.75rem",
  },
  members: {
    color: colors.mutedForeground,
    flexShrink: 0,
    fontFamily: typography.fontFamily,
    fontSize: "0.75rem",
    marginInlineStart: "auto",
  },
  id: {
    ...quietly,
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.6875rem",
    lineHeight: "1rem",
    overflowWrap: "anywhere",
    paddingInlineStart: "1.5rem",
  },
  log: {
    ...hairline,
    gap: spacing["6"],
    paddingBlock: spacing["3"],
    paddingInline: spacing["4"],
    alignItems: "center",
    borderBottomWidth: 1,
    display: "flex",
    flexWrap: "wrap",
  },
  ruler: {
    minWidth: 0,
    margin: 0,
    padding: 0,
    display: "flex",
    flexGrow: 1,
    flexWrap: "wrap",
    listStyle: "none",
  },
  run: {
    flex: "1 1 9rem",
    minWidth: 0,
  },
  tick: {
    ...hairline,
    ...quietly,
    ...ring,
    gap: 2,
    paddingBlock: spacing["1"],
    paddingInline: spacing["3"],
    alignItems: "flex-start",
    backgroundColor: { default: "transparent", ":hover": colors.background },
    borderInlineStartColor: linen.lineStrong,
    borderInlineStartWidth: 1,
    cursor: "pointer",
    display: "flex",
    flexDirection: "column",
    fontFamily: typography.fontFamily,
    justifyContent: "center",
    textAlign: "start",
    width: "100%",
  },
  tickSelected: {
    backgroundColor: linen.threadWash,
    borderInlineStartColor: linen.thread,
  },
  position: {
    ...quietly,
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.6875rem",
  },
  did: {
    ...quietly,
    color: colors.mutedForeground,
    display: "block",
    fontSize: "0.75rem",
  },
});
