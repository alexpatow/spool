"use client";

import * as stylex from "@stylexjs/stylex";
import { Fragment, useEffect, useRef } from "react";

import { Persistent, Resolution } from "@/components/explainer/identity";
import { Part, parts, timing, vars } from "@/components/explainer/part";
import { mentions, qualifiedKey, rows } from "@/components/explainer/records";
import { Scene, WIDTH } from "@/components/explainer/scene";
import { note, type SampleRecord } from "@/lib/sample";
import { colors, display, linen, typography } from "spool-ui/tokens.stylex";

const first = rows.filter((record) => ["c-1", "c-2", "c-3", "c-4", "c-5", "c-6"].includes(record.key));
const second = rows.filter((record) => !first.includes(record));

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
            Start with what you have. A CRM exports rows, and each row names a company. Spool keeps every row exactly as
            it arrived, and calls it a <strong {...strong}>record</strong>.
          </>,
          <>
            A second export arrives. Somebody typed &ldquo;Acorn Tool&rdquo;. Somebody else typed
            &ldquo;Larkfeild&rdquo;.
          </>,
          <>Some of what you know is not in rows at all. It is a note from a call, in sentences.</>,
          <>
            Acorn Tools turns up five times, spelt four ways, in two kinds of data. Is that one company or five? And is
            Halden Group in GB the same as Halden Group in the US?
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
            An <strong {...strong}>extractor</strong> reads it and marks each place that names a company. Each place is
            a <strong {...strong}>mention</strong>: where it starts, where it ends, and the name.
          </>,
          <>
            Each mention becomes a record, like a row. Its key is where it was found: characters{" "}
            {mentions[0]?.key.replace(/^.*\[(\d+)-(\d+)\]$/, "$1 to $2")} of notes-1.txt.
          </>,
          <>
            Dana Reyes is a person, and this project only knows companies, so she stays in the text. An extractor is
            code of yours in Python or TypeScript. The one a new project has asks a small model on your machine.
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
            Now there are twelve records: nine rows and three mentions. Which of them are the same company? Answering
            that is entity resolution.
          </>,
          <>
            Spool compares pairs of records with rules you write in SQL. Two names that are close enough are a match.
            Each answer is kept as a <strong {...strong}>judgement</strong>.
          </>,
          <>
            The two Halden Groups have the same name, so the match rule holds. But a no_match rule holds too, because
            their countries differ, and no_match always wins. A wrong match, turned down.
          </>,
          <>
            The judgements become <strong {...strong}>identities</strong>: one for each real company, each with an ID,
            and every record placed in one. Twelve records, six companies.
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
            Other systems store that ID, so it has to stay put. After the first file, Acorn Tools is three records and
            one ID.
          </>,
          <>A new file brings &ldquo;Acorn Tool&rdquo;. It joins. The ID does not change.</>,
          <>
            The mention from the note joins too. Still the same ID. A match places new records, and never moves the ones
            it placed before.
          </>,
          <>
            Then another file brings &ldquo;Acorn Tooling&rdquo;. The name is close enough for the rule, so it joins.
            But it is a different company.
          </>,
          <>
            A person corrects it with a split. That is a <strong {...strong}>changeset</strong>, kept in a log with an
            author and a reason, and it can be undone. Acorn Tooling gets an ID of its own. Acorn Tools keeps the one it
            had all along.
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
      <Part x={136} y={212} width={276} rotate={2.5} at={1} from={{ x: 90, rotate: 8 }} style={[parts.card, s.file]}>
        <div {...stylex.props(s.fileHead)}>samples/crm-2.csv</div>
        <div {...stylex.props(s.csv, s.csvHead)}>id,name,country</div>
        {second.map((record) => (
          <CsvRow key={record.key} record={record} />
        ))}
      </Part>
      <Part x={4} y={364} width={412} rotate={-1} at={2} from={{ y: 80, rotate: -4 }} style={[parts.card, s.file]}>
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
              <span key={index} {...stylex.props(s.person, s.transition)} style={timing({ lit: { at: 3 } })}>
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
          <span {...stylex.props(s.foundKey)}>{qualifiedKey(record)}</span>
          <span {...stylex.props(parts.part, s.badge)} style={timing({ at: 3, from: { x: -8 }, delay: 120 * index })}>
            record
          </span>
        </div>
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
});
