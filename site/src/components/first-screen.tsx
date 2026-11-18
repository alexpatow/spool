import * as stylex from "@stylexjs/stylex";
import { Fragment } from "react";

import { Mark } from "@/components/brand";
import { identities, log, note, records } from "@/lib/sample";
import { breakpoints } from "@/styles/layout.stylex";
import { colors, display, linen, spacing, typography } from "@/styles/shelf/tokens.stylex";

/** The heights the threads are drawn against, in pixels: a column's heading, and one row. */
const HEAD = 36;
const ROW = 28;
/** The identity whose thread is drawn in the one colour. */
const SHOWN = 0;

const rows = records.filter((record) => record.source === "crm");
const found = records.filter((record) => record.source === "notes");
/** Room under the last record, which the note beside it needs. */
const FOOT = 84;
const BODY = records.length * ROW + FOOT;
const card = BODY / identities.length;

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

function members(identity: number) {
  const held = records.filter((record) => record.identity === identity);
  const sources = new Set(held.map((record) => record.source)).size;
  return `${held.length} ${held.length === 1 ? "record" : "records"} · ${sources} ${sources === 1 ? "source" : "sources"}`;
}

/**
 * The front page's first screen: what came in, the records it became, and the
 * identities they were placed in, with a thread from each record to its
 * identity. It is the sample project of Getting started after its last step.
 */
export function FirstScreen() {
  const height = HEAD + BODY;
  return (
    <figure {...stylex.props(styles.figure)} aria-label="The sample project: rows and a note, the records they became, and their identities">
      <div {...stylex.props(styles.panel)}>
        <div {...stylex.props(styles.column, styles.sources)}>
          <div {...stylex.props(styles.head)}>
            <span {...stylex.props(styles.label)}>Rows</span>
            <span {...stylex.props(styles.aside)}>samples/crm-1.csv, crm-2.csv</span>
          </div>
          {rows.map((record) => (
            <div key={record.key} {...stylex.props(styles.line)}>
              {record.key},{record.name},{record.country}
            </div>
          ))}
          <div {...stylex.props(styles.head, styles.noteHead)}>
            <span {...stylex.props(styles.label)}>Text</span>
            <span {...stylex.props(styles.aside)}>{note.file}</span>
          </div>
          <p {...stylex.props(styles.note)}>
            {marked().map((part, index) =>
              part.identity === undefined ? (
                <Fragment key={index}>{part.text}</Fragment>
              ) : (
                <mark key={index} {...stylex.props(styles.mention, part.identity === SHOWN && styles.mentionShown)}>
                  {part.text}
                </mark>
              ),
            )}
          </p>
        </div>

        <div {...stylex.props(styles.column, styles.records)}>
          <div {...stylex.props(styles.head)}>
            <span {...stylex.props(styles.label)}>Records</span>
            <span {...stylex.props(styles.aside)}>kept as they arrived</span>
            <span {...stylex.props(styles.count)}>{records.length}</span>
          </div>
          <table {...stylex.props(styles.table)}>
            <thead {...stylex.props(styles.hidden)}>
              <tr>
                <th>Source and key</th>
                <th>Name</th>
                <th>Identity</th>
              </tr>
            </thead>
            <tbody>
              {records.map((record) => (
                <tr key={`${record.source}:${record.key}`} {...stylex.props(styles.row)}>
                  <td {...stylex.props(styles.cell, styles.key)}>
                    <span {...stylex.props(styles.source)}>{record.source}:</span>
                    {record.key}
                  </td>
                  <td {...stylex.props(styles.cell, styles.name)}>{record.name}</td>
                  <td {...stylex.props(styles.cell, styles.placed, record.identity === SHOWN && styles.thread)}>
                    …{identities[record.identity]?.id.slice(-4)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <p {...stylex.props(styles.foot)}>
            {rows.length} from rows, {found.length} from the note. A record from text is keyed by where it was found.
          </p>
        </div>

        <svg
          {...stylex.props(styles.threads)}
          viewBox={`0 0 100 ${height}`}
          height={height}
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          {records.map((record, index) => {
            const from = HEAD + index * ROW + ROW / 2;
            const to = HEAD + record.identity * card + card / 2;
            return (
              <path
                key={index}
                d={`M0 ${from}C50 ${from} 50 ${to} 100 ${to}`}
                fill="none"
                vectorEffect="non-scaling-stroke"
                {...stylex.props(styles.strand, record.identity === SHOWN && styles.strandShown)}
              />
            );
          })}
        </svg>

        <div {...stylex.props(styles.column, styles.identities)}>
          <div {...stylex.props(styles.head)}>
            <span {...stylex.props(styles.label)}>Identities</span>
            <span {...stylex.props(styles.aside)}>one ID each</span>
            <span {...stylex.props(styles.count)}>{identities.length}</span>
          </div>
          {identities.map((identity, index) => (
            <div key={identity.id} {...stylex.props(styles.identity)} style={{ height: card }}>
              <div {...stylex.props(styles.identityTop)}>
                <Mark width={16} height={16} {...stylex.props(styles.mark, index === SHOWN && styles.thread)} />
                <span {...stylex.props(styles.identityName)}>{identity.name}</span>
                {"detail" in identity ? <span {...stylex.props(styles.detail)}>{identity.detail}</span> : null}
                <span {...stylex.props(styles.members)}>{members(index)}</span>
              </div>
              <div {...stylex.props(styles.id, index === SHOWN && styles.thread)}>{identity.id}</div>
            </div>
          ))}
        </div>
      </div>

      <div {...stylex.props(styles.log)}>
        <div {...stylex.props(styles.logLabel)}>
          <span {...stylex.props(styles.label)}>Changeset log</span>
          <span {...stylex.props(styles.aside)}>append-only</span>
        </div>
        <ol {...stylex.props(styles.ruler)}>
          {log.map((entry) => (
            <li key={entry.after} {...stylex.props(styles.tick)}>
              <span {...stylex.props(styles.position)}>match after {entry.after}</span>
              <span {...stylex.props(styles.did)}>{entry.did}</span>
            </li>
          ))}
          <li {...stylex.props(styles.tick, styles.next)}>
            <span {...stylex.props(styles.did)}>next</span>
          </li>
        </ol>
      </div>
    </figure>
  );
}

const hairline = { borderColor: colors.border, borderStyle: "solid", borderWidth: 0 } as const;

const styles = stylex.create({
  figure: {
    ...hairline,
    margin: 0,
    borderWidth: 1,
    backgroundColor: colors.card,
    textAlign: "start",
  },
  panel: {
    display: "grid",
    gridTemplateColumns: {
      default: "minmax(0, 1.05fr) minmax(0, 1.45fr) 5rem minmax(0, 1.2fr)",
      [breakpoints.narrow]: "1fr",
    },
  },
  column: {
    minWidth: 0,
  },
  sources: {
    ...hairline,
    borderInlineEndWidth: { default: 1, [breakpoints.narrow]: 0 },
    borderBottomWidth: { default: 0, [breakpoints.narrow]: 1 },
    backgroundColor: colors.background,
  },
  records: {
    ...hairline,
    borderBottomWidth: { default: 0, [breakpoints.narrow]: 1 },
  },
  identities: {
    ...hairline,
    borderInlineStartWidth: { default: 1, [breakpoints.narrow]: 0 },
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
  label: {
    color: colors.foreground,
    fontSize: "0.6875rem",
    fontWeight: typography.fontWeightMedium,
    letterSpacing: "0.08em",
    textTransform: "uppercase",
  },
  aside: {
    color: linen.faint,
    fontSize: "0.75rem",
  },
  count: {
    color: linen.faint,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.75rem",
    marginInlineStart: "auto",
  },
  noteHead: {
    borderTopWidth: 1,
    height: ROW,
  },
  foot: {
    margin: 0,
    paddingInline: spacing["4"],
    paddingTop: spacing["2"],
    color: linen.faint,
    fontSize: "0.75rem",
    lineHeight: "1.125rem",
  },
  line: {
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
  note: {
    margin: 0,
    paddingBlock: spacing["2"],
    paddingInline: spacing["4"],
    color: colors.foreground,
    fontSize: "0.8125rem",
    lineHeight: "1.25rem",
  },
  mention: {
    backgroundColor: "transparent",
    color: "inherit",
    textDecorationColor: linen.lineStrong,
    textDecorationLine: "underline",
    textDecorationThickness: 1,
    textUnderlineOffset: 4,
  },
  mentionShown: {
    backgroundColor: linen.threadWash,
    color: linen.thread,
    textDecorationColor: linen.thread,
  },
  table: {
    borderCollapse: "collapse",
    tableLayout: "fixed",
    width: "100%",
  },
  hidden: {
    position: "absolute",
    clip: "rect(0 0 0 0)",
    overflow: "hidden",
    height: 1,
    width: 1,
  },
  row: {
    height: ROW,
  },
  cell: {
    padding: 0,
    paddingInlineStart: spacing["4"],
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.71875rem",
    lineHeight: `${ROW}px`,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  source: {
    color: linen.faint,
  },
  key: {
    color: colors.mutedForeground,
    width: "13.5rem",
  },
  name: {
    color: colors.foreground,
    fontFamily: typography.fontFamily,
    fontSize: "0.8125rem",
  },
  placed: {
    color: linen.faint,
    display: { default: "none", [breakpoints.narrow]: "table-cell" },
    width: "3.5rem",
  },
  thread: {
    color: linen.thread,
  },
  threads: {
    display: { default: "block", [breakpoints.narrow]: "none" },
    width: "100%",
  },
  strand: {
    stroke: linen.lineStrong,
    strokeWidth: 1,
  },
  strandShown: {
    stroke: linen.thread,
    strokeWidth: 1.25,
  },
  identity: {
    ...hairline,
    gap: 2,
    paddingInline: spacing["4"],
    borderBottomWidth: { default: 1, ":last-child": 0 },
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    justifyContent: "center",
  },
  identityTop: {
    gap: spacing["2"],
    alignItems: "baseline",
    display: "flex",
    minWidth: 0,
  },
  mark: {
    alignSelf: "center",
    color: linen.faint,
    flexShrink: 0,
  },
  identityName: {
    color: colors.foreground,
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
    color: linen.faint,
    fontSize: "0.75rem",
  },
  members: {
    color: linen.faint,
    flexShrink: 0,
    fontSize: "0.75rem",
    marginInlineStart: "auto",
  },
  id: {
    color: linen.faint,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.6875rem",
    lineHeight: "1rem",
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
    paddingInlineStart: "1.5rem",
  },
  log: {
    ...hairline,
    gap: spacing["6"],
    paddingBlock: spacing["3"],
    paddingInline: spacing["4"],
    alignItems: "center",
    borderTopWidth: 1,
    display: "flex",
    flexWrap: "wrap",
  },
  logLabel: {
    gap: spacing["2"],
    alignItems: "center",
    display: "flex",
    flexShrink: 0,
  },
  ruler: {
    margin: 0,
    padding: 0,
    display: "flex",
    flexGrow: 1,
    flexWrap: "wrap",
    listStyle: "none",
  },
  tick: {
    gap: 2,
    paddingBlock: 2,
    paddingInline: spacing["3"],
    borderInlineStartColor: linen.thread,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: 1,
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
  },
  next: {
    borderInlineStartColor: linen.lineStrong,
    borderInlineStartStyle: "dashed",
    flexGrow: 0,
    justifyContent: "center",
  },
  position: {
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.6875rem",
  },
  did: {
    color: colors.mutedForeground,
    fontSize: "0.75rem",
  },
});
