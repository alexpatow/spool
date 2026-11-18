"use client";

import * as stylex from "@stylexjs/stylex";
import { Fragment, useEffect, useRef } from "react";

import { Mark } from "@/components/brand";
import { Part, parts, timing, vars } from "@/components/explainer/part";
import { HEIGHT, Scene, WIDTH } from "@/components/explainer/scene";
import { identities, note, records, type SampleRecord } from "@/lib/sample";
import { colors, display, linen, typography } from "@/styles/shelf/tokens.stylex";

const rows = records.filter((record) => record.source === "crm");
const mentions = records.filter((record) => record.source === "notes");
const first = rows.filter((record) => ["c-1", "c-2", "c-3", "c-4", "c-5", "c-6"].includes(record.key));
const second = rows.filter((record) => !first.includes(record));
const acorn = identities[0];

/** How `spool show records` names a record: its source and its key. */
function named(record: SampleRecord): string {
  return `${record.source}:${record.key}`;
}

const isAcorn = (name: string) => name.toLowerCase().startsWith("acorn");

/** The note, cut where the extractor found a company, and where it named a person. */
function pieces() {
  const marks = [
    ...mentions.map((record) => ({ name: record.name, company: true })),
    { name: "Dana Reyes", company: false },
  ]
    .map((mark) => ({ ...mark, start: note.text.indexOf(mark.name) }))
    .sort((a, b) => a.start - b.start);
  const out: { text: string; mention?: number; person?: boolean }[] = [];
  let at = 0;
  for (const mark of marks) {
    out.push({ text: note.text.slice(at, mark.start) });
    out.push(
      mark.company
        ? { text: mark.name, mention: mentions.findIndex((record) => record.name === mark.name) }
        : { text: mark.name, person: true },
    );
    at = mark.start + mark.name.length;
  }
  out.push({ text: note.text.slice(at) });
  return out;
}

export function Explainer() {
  const strong = stylex.props(s.strong);
  return (
    <>
      <Scene
        id="messy-input"
        number={1}
        name="Messy input"
        captions={[
          <>
            Start with what you have. A CRM exports rows, and each row names a company. Spool keeps
            every row exactly as it arrived, and calls it a <strong {...strong}>record</strong>.
          </>,
          <>
            A second export arrives. Somebody typed &ldquo;Acorn Tool&rdquo;. Somebody else typed
            &ldquo;Larkfeild&rdquo;.
          </>,
          <>Some of what you know is not in rows at all. It is a note from a call, in sentences.</>,
          <>
            Acorn Tools turns up five times, spelt four ways, in two kinds of data. Is that one
            company or five? And is Halden Group in GB the same as Halden Group in the US?
          </>,
        ]}
      >
        <Input />
      </Scene>

      <Scene
        id="extraction"
        number={2}
        name="Extraction"
        captions={[
          <>Text names things too, just not in columns. Here is that note.</>,
          <>
            An <strong {...strong}>extractor</strong> reads it and marks each place that names a
            company. Each place is a <strong {...strong}>mention</strong>: where it starts, where it
            ends, and the name.
          </>,
          <>
            Each mention becomes a record, like a row. Its key is where it was found: characters{" "}
            {mentions[0]?.key.replace(/^.*\[(\d+)-(\d+)\]$/, "$1 to $2")} of notes-1.txt.
          </>,
          <>
            Dana Reyes is a person, and this project only knows companies, so she stays in the
            text. An extractor is code of yours in Python or TypeScript. The one a new project has
            asks a small model on your machine.
          </>,
        ]}
      >
        <Extraction />
      </Scene>

      <Scene
        id="resolution"
        number={3}
        name="Resolution"
        captions={[
          <>
            Now there are twelve records: nine rows and three mentions. Which of them are the same
            company? Answering that is entity resolution.
          </>,
          <>
            Spool compares pairs of records with rules you write in SQL. Two names that are close
            enough are a match. Each answer is kept as a <strong {...strong}>judgement</strong>.
          </>,
          <>
            The two Halden Groups have the same name, so the match rule holds. But a no_match rule
            holds too, because their countries differ, and no_match always wins. A wrong match,
            turned down.
          </>,
          <>
            The judgements become <strong {...strong}>identities</strong>: one for each real
            company, each with an ID, and every record placed in one. Twelve records, six
            companies.
          </>,
        ]}
      >
        <Resolution />
      </Scene>

      <Scene
        id="persistent-identity"
        number={4}
        name="Persistent identity"
        captions={[
          <>
            Other systems store that ID, so it has to stay put. After the first file, Acorn Tools
            is three records and one ID.
          </>,
          <>A new file brings &ldquo;Acorn Tool&rdquo;. It joins. The ID does not change.</>,
          <>
            The mention from the note joins too. Still the same ID. A match places new records, and
            never moves the ones it placed before.
          </>,
          <>
            Then another file brings &ldquo;Acorn Tooling&rdquo;. The name is close enough for the
            rule, so it joins. But it is a different company.
          </>,
          <>
            A person corrects it with a split. That is a <strong {...strong}>changeset</strong>,
            kept in a log with an author and a reason, and it can be undone. Acorn Tooling gets an
            ID of its own. Acorn Tools keeps the one it had all along.
          </>,
        ]}
      >
        <Persistent />
      </Scene>
    </>
  );
}

/* ------------------------------------------------------------------ */
/* 1. Messy input                                                      */
/* ------------------------------------------------------------------ */

function Input() {
  return (
    <>
      <Part x={0} y={20} width={276} rotate={-2.5} style={[parts.card, s.file]} from={{ y: 24 }}>
        <div {...stylex.props(s.fileHead)}>samples/crm-1.csv</div>
        <div {...stylex.props(s.csv, s.csvHead)}>id,name,country</div>
        {first.map((record) => (
          <CsvRow key={record.key} record={record} />
        ))}
      </Part>
      <Part
        x={136}
        y={212}
        width={276}
        rotate={2.5}
        at={1}
        from={{ x: 90, rotate: 8 }}
        style={[parts.card, s.file]}
      >
        <div {...stylex.props(s.fileHead)}>samples/crm-2.csv</div>
        <div {...stylex.props(s.csv, s.csvHead)}>id,name,country</div>
        {second.map((record) => (
          <CsvRow key={record.key} record={record} />
        ))}
      </Part>
      <Part
        x={4}
        y={364}
        width={412}
        rotate={-1}
        at={2}
        from={{ y: 80, rotate: -4 }}
        style={[parts.card, s.file]}
      >
        <div {...stylex.props(s.fileHead)}>{note.file}</div>
        <p {...stylex.props(s.noteSmall)}>
          {pieces().map((piece, index) =>
            piece.mention !== undefined && isAcorn(piece.text) ? (
              <span
                key={index}
                {...stylex.props(parts.litStrong, parts.litWash, s.transition)}
                style={timing({ lit: { at: 3 }, delay: 300 })}
              >
                {piece.text}
              </span>
            ) : (
              <Fragment key={index}>{piece.text}</Fragment>
            ),
          )}
        </p>
      </Part>
    </>
  );
}

function CsvRow({ record }: { record: SampleRecord }) {
  const lit = isAcorn(record.name);
  return (
    <div
      {...stylex.props(s.csv, lit && parts.litStrong, lit && parts.litWash, s.transition)}
      style={timing({ lit: { at: lit ? 3 : 999 }, delay: 120 * rows.indexOf(record) })}
    >
      {record.key},{record.name},{record.country}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* 2. Extraction                                                       */
/* ------------------------------------------------------------------ */

const CHIP_Y = 236;
const CHIP_PITCH = 84;

function Extraction() {
  const marks = useRef<(HTMLElement | null)[]>([]);
  const chips = useRef<(HTMLDivElement | null)[]>([]);

  // Each record starts where its mention is in the text. That depends on how
  // the text wraps, so it is measured once the typefaces are in.
  useEffect(() => {
    let gone = false;
    const place = () => {
      if (gone) return;
      marks.current.forEach((mark, index) => {
        const chip = chips.current[index];
        const box = mark?.offsetParent as HTMLElement | null;
        if (!mark || !chip || !box) return;
        const x = box.offsetLeft + mark.offsetLeft;
        const y = box.offsetTop + mark.offsetTop;
        chip.style.setProperty("--fx", String(x - chip.offsetLeft));
        chip.style.setProperty("--fy", String(y - chip.offsetTop));
      });
    };
    place();
    void document.fonts.ready.then(place);
    return () => {
      gone = true;
    };
  }, []);

  return (
    <>
      <Part x={0} y={0} width={WIDTH} style={[parts.card, s.file]}>
        <div {...stylex.props(s.fileHead)}>{note.file}</div>
        <p {...stylex.props(s.noteLarge)}>
          {pieces().map((piece, index) =>
            piece.mention !== undefined ? (
              <mark
                key={index}
                ref={(element) => {
                  marks.current[piece.mention ?? 0] = element;
                }}
                {...stylex.props(s.mention, parts.litStrong, parts.litWash, s.transition)}
                style={timing({ lit: { at: 1 }, delay: 350 + 300 * (piece.mention ?? 0) })}
              >
                {piece.text}
              </mark>
            ) : piece.person ? (
              <span
                key={index}
                {...stylex.props(s.person, s.transition)}
                style={timing({ lit: { at: 3 } })}
              >
                {piece.text}
              </span>
            ) : (
              <Fragment key={index}>{piece.text}</Fragment>
            ),
          )}
        </p>
      </Part>

      <Part
        x={0}
        y={30}
        width={3}
        height={168}
        at={1}
        until={2}
        move={{ x: WIDTH - 3, at: 1 }}
        duration={1300}
        style={s.scan}
      />

      <Part x={8} y={206} at={3} from={{ y: -6 }} style={s.aside}>
        Dana Reyes: a person, not a company. No record.
      </Part>

      {mentions.map((record, index) => (
        <div
          key={record.key}
          ref={(element) => {
            chips.current[index] = element;
          }}
          {...stylex.props(parts.part, parts.card, s.found)}
          style={{
            ...timing({ at: 2, from: { scale: 0.55 }, delay: 140 * index, duration: 900 }),
            ...vars({ "--r": 0 }),
            left: 0,
            top: CHIP_Y + index * CHIP_PITCH + 16,
            width: WIDTH,
          }}
        >
          <span {...stylex.props(s.foundName)}>{record.name}</span>
          <span {...stylex.props(s.foundType)}>company</span>
          <span {...stylex.props(s.foundKey)}>{named(record)}</span>
          <span
            {...stylex.props(parts.part, s.badge)}
            style={timing({ at: 3, from: { x: -8 }, delay: 120 * index })}
          >
            record
          </span>
        </div>
      ))}
    </>
  );
}

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

function Resolution() {
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
                pair.match
                  ? { at: 1, until: 3, delay: 70 * order, duration: 700 }
                  : { at: 2, until: 3, duration: 900 },
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
            key={named(record)}
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
  { actor: "engine", did: `notes-1.txt: place ${mentions[0] ? named(mentions[0]) : ""}` },
  { actor: "engine", did: "crm-3.csv: place crm:c-10" },
  { actor: "user", did: "\u201cAcorn Tooling is another company\u201d: split" },
];

const ROW_Y = 112;
const ROW_PITCH = 30;
const SPLIT_Y = 318;
const LOG_Y = 420;

function Persistent() {
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
  strong: {
    color: colors.foreground,
    fontWeight: typography.fontWeightMedium,
  },
  transition: {
    transitionDelay: "calc(var(--motion, 0) * var(--delay) * 1ms)",
    transitionDuration: "calc(var(--motion, 0) * 600ms)",
    transitionProperty: "color, background-color, text-decoration-color",
    transitionTimingFunction: "ease-out",
  },
  file: {
    paddingBottom: 8,
    boxShadow: "0 1px 2px rgb(0 0 0 / 4%), 0 8px 24px -12px rgb(0 0 0 / 18%)",
  },
  fileHead: {
    borderBottomColor: colors.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: 11,
    lineHeight: "30px",
    marginBottom: 6,
    paddingInline: 12,
  },
  csv: {
    color: colors.foreground,
    fontFamily: typography.fontFamilyMono,
    fontSize: 12,
    lineHeight: "22px",
    paddingInline: 12,
    whiteSpace: "nowrap",
  },
  csvHead: {
    color: linen.faint,
  },
  noteSmall: {
    margin: 0,
    paddingBlock: 2,
    paddingInline: 12,
    color: colors.foreground,
    fontSize: 13,
    lineHeight: "21px",
  },
  noteLarge: {
    margin: 0,
    paddingBlock: 6,
    paddingInline: 16,
    color: colors.foreground,
    fontSize: 15,
    lineHeight: "26px",
  },
  mention: {
    textDecorationColor: `color-mix(in oklab, ${linen.thread} calc(min(clamp(0, calc(var(--step, 99) - var(--lat) + 1), 1), 1) * 100%), transparent)`,
    textDecorationLine: "underline",
    textDecorationThickness: 1.5,
    textUnderlineOffset: 4,
  },
  person: {
    textDecorationColor: `color-mix(in oklab, ${linen.faint} calc(clamp(0, calc(var(--step, 99) - var(--lat) + 1), 1) * 100%), transparent)`,
    textDecorationLine: "underline",
    textDecorationStyle: "dashed",
    textUnderlineOffset: 4,
  },
  scan: {
    backgroundImage: `linear-gradient(to bottom, transparent, ${linen.thread}, transparent)`,
    boxShadow: `0 0 18px 4px color-mix(in oklab, ${linen.thread} 35%, transparent)`,
  },
  aside: {
    color: colors.mutedForeground,
    fontSize: 12,
    whiteSpace: "nowrap",
  },
  found: {
    paddingBlock: 10,
    paddingInline: 14,
    display: "grid",
    gridTemplateColumns: "1fr auto",
    alignItems: "center",
    rowGap: 6,
    height: 72,
    transformOrigin: "top left",
  },
  foundName: {
    color: colors.foreground,
    fontFamily: display.fontFamily,
    fontSize: 20,
    fontWeight: linen.displayWeight,
    lineHeight: "24px",
  },
  foundType: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: 11,
    alignSelf: "center",
  },
  foundKey: {
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: 12,
  },
  badge: {
    position: "relative",
    borderColor: linen.thread,
    borderStyle: "solid",
    borderWidth: 1,
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: 10,
    justifySelf: "end",
    letterSpacing: "0.06em",
    lineHeight: "16px",
    paddingInline: 6,
    textTransform: "uppercase",
  },
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
