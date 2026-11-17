// The pieces the screens are made of.

import * as stylex from "@stylexjs/stylex";
import { useEffect, useState } from "react";
import type { ReactNode } from "react";

import { SpoolError } from "./api.ts";
import type { Op } from "./api.ts";
import { Button } from "./components/ui/button";
import { Checkbox } from "./components/ui/checkbox";
import { CopyButton } from "./components/ui/copy-button";
import { Input } from "./components/ui/input";
import * as NativeSelect from "./components/ui/native-select";
import { Spinner } from "./components/ui/spinner";
import * as ShelfTable from "./components/ui/table";
import { describeOp, plain, short, tail, when } from "./format.ts";
import type { Part } from "./format.ts";
import type { Loaded, Pages } from "./hooks.ts";
import { ArrowRightIcon, CircleAlertIcon, CircleCheckIcon, InfoIcon } from "./icons.tsx";
import { href, navigate } from "./router.ts";
import type { QueryValue } from "./router.ts";
import { colors, radius, spacing, typography } from "./styles/shelf/tokens.stylex";
import { tones } from "./styles/tones.stylex";

export function message(error: unknown): string {
  if (error instanceof SpoolError) return error.message;
  if (error instanceof Error) return `The server did not answer: ${error.message}`;
  return String(error);
}

export type Tone = "good" | "waits" | "bad" | "plain";

/** Something the reader should not miss: what a change came to, or why it was not made. */
export function Notice({
  tone = "plain",
  role,
  children,
}: {
  tone?: Tone;
  role?: "alert" | "status";
  children: ReactNode;
}) {
  const Icon = tone === "good" ? CircleCheckIcon : tone === "plain" ? InfoIcon : CircleAlertIcon;
  return (
    <div data-part="notice" data-tone={tone} {...stylex.props(styles.notice, noticeTones[tone])}>
      <Icon {...stylex.props(styles.noticeIcon)} />
      <div role={role} {...stylex.props(styles.noticeBody)}>
        {children}
      </div>
    </div>
  );
}

export function Failure({ error }: { error: unknown }) {
  return (
    <Notice tone="bad" role="alert">
      {message(error)}
    </Notice>
  );
}

/** What is said in passing: it explains, and nothing depends on it. */
export function Quiet({ children }: { children: ReactNode }) {
  return <p {...stylex.props(styles.quiet)}>{children}</p>;
}

/** A paragraph of the page. */
export function Text({ children }: { children: ReactNode }) {
  return <p {...stylex.props(styles.text)}>{children}</p>;
}

/** What someone gave as their reason, in their words. */
export function Reason({ children }: { children: ReactNode }) {
  return (
    <span data-part="reason" {...stylex.props(styles.reason)}>
      “{children}”
    </span>
  );
}

/** A name, a key or a value as the store holds it. */
export function Code({ children }: { children: ReactNode }) {
  return <code {...stylex.props(styles.code)}>{children}</code>;
}

/** A list of short lines. */
export function Lines({ children }: { children: ReactNode }) {
  return <ul {...stylex.props(styles.lines)}>{children}</ul>;
}

/** What was read, or that it is being read, or why it could not be. */
export function Load<T>({ state, children }: { state: Loaded<T>; children: (data: T) => ReactNode }) {
  if (state.status === "loading") {
    return (
      <p {...stylex.props(styles.quiet, styles.reading)}>
        <Spinner role="presentation" aria-label={undefined} aria-hidden /> Loading…
      </p>
    );
  }
  if (state.status === "failed") return <Failure error={state.error} />;
  return <>{children(state.data)}</>;
}

export function Page({
  title,
  kind,
  about,
  actions,
  children,
}: {
  title: string;
  /** What kind of thing the page shows, when its title is the thing's own name. */
  kind?: ReactNode;
  about?: ReactNode;
  /** What can be done to the whole of what the page shows. */
  actions?: ReactNode;
  children: ReactNode;
}) {
  useEffect(() => {
    document.title = `${title} · Spool studio`;
  }, [title]);
  return (
    <section {...stylex.props(styles.page)}>
      <header {...stylex.props(styles.pageHead)}>
        <div {...stylex.props(styles.pageTitles)}>
          {kind ? <div {...stylex.props(styles.kind)}>{kind}</div> : null}
          <h1 {...stylex.props(styles.h1)}>{title}</h1>
          {about ? <div {...stylex.props(styles.about)}>{about}</div> : null}
        </div>
        {actions ? <div {...stylex.props(styles.actions)}>{actions}</div> : null}
      </header>
      {children}
    </section>
  );
}

export function Section({
  title,
  count,
  about,
  actions,
  children,
}: {
  title: string;
  /** How many of them there are, beside the title. */
  count?: number;
  about?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section {...stylex.props(styles.section)}>
      <header {...stylex.props(styles.sectionHead)}>
        <div {...stylex.props(styles.sectionTitles)}>
          <h2 {...stylex.props(styles.h2)}>
            {title}
            {count !== undefined ? (
              <>
                {" "}
                <span data-part="count" {...stylex.props(styles.count)}>
                  {count}
                </span>
              </>
            ) : null}
          </h2>
          {about ? <div {...stylex.props(styles.about)}>{about}</div> : null}
        </div>
        {actions ? <div {...stylex.props(styles.actions)}>{actions}</div> : null}
      </header>
      {children}
    </section>
  );
}

export interface Choosing {
  label: string;
  address: string;
  current: boolean;
  /** How many there are of it, when the screen knows. */
  count?: number;
}

/** The ways one list can be narrowed, of which one is chosen. Each is a link, so the choice is in the address. */
export function Choose({ label, choices }: { label: string; choices: Choosing[] }) {
  return (
    <ul aria-label={label} {...stylex.props(styles.choose)}>
      {choices.map((choice) => (
        <li key={choice.label} {...stylex.props(styles.choiceItem)}>
          <a
            href={choice.address}
            aria-current={choice.current ? "true" : undefined}
            {...stylex.props(styles.choice, choice.current && styles.choiceCurrent)}
          >
            {choice.label}
            {choice.count !== undefined ? (
              <span data-part="count" {...stylex.props(styles.count)}>
                {choice.count}
              </span>
            ) : null}
          </a>
        </li>
      ))}
    </ul>
  );
}

/** A surface with a border, for what belongs together. */
export function Panel({ children, flush = false }: { children: ReactNode; flush?: boolean }) {
  return <div {...stylex.props(styles.panel, !flush && styles.panelPadded)}>{children}</div>;
}

/** Things in a row, which goes on to the next line when there is no room. */
export function Row({ children, end = false }: { children: ReactNode; end?: boolean }) {
  return <div {...stylex.props(styles.row, end && styles.rowEnd)}>{children}</div>;
}

/** Things under each other. */
export function Stack({ children }: { children: ReactNode }) {
  return <div {...stylex.props(styles.stack)}>{children}</div>;
}

/** Label and value, one pair per row. A value that is `null` or `undefined` leaves its row out. */
export function Fields({ rows }: { rows: Array<[string, ReactNode]> }) {
  return (
    <dl {...stylex.props(styles.fields)}>
      {rows
        .filter(([, value]) => value !== null && value !== undefined)
        .map(([label, value]) => (
          <div key={label} {...stylex.props(styles.field)}>
            <dt {...stylex.props(styles.term)}>{label}</dt>
            <dd {...stylex.props(styles.value)}>{value}</dd>
          </div>
        ))}
    </dl>
  );
}

export interface Column<T> {
  title: string;
  cell: (row: T) => ReactNode;
  /** Numbers are set to the right. */
  number?: boolean;
  /** Long text goes on to the next line where others stay on one. */
  wrap?: boolean;
}

export function Table<T>({
  columns,
  rows,
  rowKey,
  empty,
  label,
}: {
  columns: Array<Column<T>>;
  rows: T[];
  rowKey: (row: T) => string | number;
  /** What to say when there is nothing to list. */
  empty: string;
  /** What the table lists, for a reader who cannot see what is above it. */
  label?: string;
}) {
  if (rows.length === 0) {
    return (
      <div data-part="empty" {...stylex.props(styles.panel, styles.empty)}>
        {empty}
      </div>
    );
  }
  return (
    <div {...stylex.props(styles.panel)}>
      <ShelfTable.Root aria-label={label}>
        <ShelfTable.Header>
          <ShelfTable.Row style={styles.headRow}>
            {columns.map((column) => (
              <ShelfTable.Head
                key={column.title}
                style={[styles.head, column.number && styles.number, column.title === "" && styles.tick]}
              >
                {column.title}
              </ShelfTable.Head>
            ))}
          </ShelfTable.Row>
        </ShelfTable.Header>
        <ShelfTable.Body>
          {rows.map((row) => (
            <ShelfTable.Row key={rowKey(row)} style={styles.bodyRow}>
              {columns.map((column) => (
                <ShelfTable.Cell
                  key={column.title}
                  style={[styles.cell, column.number && styles.number, column.wrap && styles.wrap]}
                >
                  {column.cell(row)}
                </ShelfTable.Cell>
              ))}
            </ShelfTable.Row>
          ))}
        </ShelfTable.Body>
      </ShelfTable.Root>
    </div>
  );
}

/** The button under a list that reads its next page. */
export function More({ pages }: { pages: Pick<Pages<unknown>, "more" | "reading" | "readMore"> }) {
  if (!pages.more) return null;
  return (
    <div {...stylex.props(styles.more)}>
      <Button variant="outline" size="sm" onClick={pages.readMore} disabled={pages.reading}>
        {pages.reading ? "Loading…" : "Show more"}
      </Button>
    </div>
  );
}

/** A list read in pages, with the button that reads the next one. */
export function PagedTable<T>({
  pages,
  ...table
}: {
  pages: Pages<T>;
  columns: Array<Column<T>>;
  rowKey: (row: T) => string | number;
  empty: string;
  label?: string;
}) {
  return (
    <Load state={pages.loaded}>
      {(rows) => (
        <>
          <Table rows={rows} {...table} />
          <More pages={pages} />
        </>
      )}
    </Load>
  );
}

const TONES: Record<string, Tone> = {
  succeeded: "good",
  committed: "good",
  resolved: "good",
  match: "good",
  holds: "good",
  same_identity: "good",
  must_link: "good",
  open: "waits",
  drafted: "waits",
  running: "waits",
  unsure: "waits",
  unassigned: "waits",
  possible_merge: "waits",
  failed: "bad",
  rejected: "bad",
  no_match: "bad",
  conflict: "bad",
  cannot_link: "bad",
  different_identities: "bad",
};

/** One of the API's words, such as a status or a verdict, coloured by what it means. */
export function Word({ value, label, tone }: { value: string; label?: string; tone?: Tone }) {
  const said = tone ?? TONES[value] ?? "plain";
  return (
    <span data-part="word" data-tone={said} {...stylex.props(styles.word, wordTones[said])}>
      <span {...stylex.props(styles.mark, markTones[said])} />
      {label ?? value.replaceAll("_", " ")}
    </span>
  );
}

const SOURCES = ["source1", "source2", "source3", "source4", "source5", "source6"] as const satisfies ReadonlyArray<
  keyof typeof sourceMarks
>;

/** The colour a source keeps on every screen, so that its records are told from another's at a glance. */
function sourceMark(source: string) {
  let sum = 0;
  for (const letter of source) sum = (sum * 31 + letter.codePointAt(0)!) % 9973;
  return sourceMarks[SOURCES[sum % SOURCES.length]!];
}

/** Where records came from: the source's name, and how many of them when that is said. */
export function Source({ name, records }: { name: string; records?: number }) {
  return (
    <span data-part="source" {...stylex.props(styles.source)}>
      <span {...stylex.props(styles.sourceMark, sourceMark(name))} />
      {name}
      {records !== undefined ? <span {...stylex.props(styles.sourceCount)}>{records}</span> : null}
    </span>
  );
}

export function Sources({ sources }: { sources: Array<{ source: string; records: number }> }) {
  return (
    <span {...stylex.props(styles.sources)}>
      {sources.map(({ source, records }) => (
        <Source key={source} name={source} records={records} />
      ))}
    </span>
  );
}

export type Kind = "record" | "identity" | "document" | "draft" | "run" | "review" | "log";

const SCREENS: Record<Kind, string> = {
  record: "records",
  identity: "identities",
  document: "documents",
  draft: "drafts",
  run: "runs",
  review: "review",
  log: "log",
};

/** A link to the screen of one thing. A long ID is shortened and shown whole on hover. */
export function Ref({ kind, id, label }: { kind: Kind; id: string | number; label?: ReactNode }) {
  const text = String(id);
  const bare = label === undefined || label === null;
  return (
    <a
      href={href([SCREENS[kind], text])}
      title={text}
      data-kind={kind}
      {...stylex.props(styles.link, bare && typeof id === "string" && styles.id)}
    >
      {bare ? short(text) : label}
    </a>
  );
}

/** A time to the second, where the reader is. The time as the server gave it shows on hover. */
export function When({ at }: { at: string }) {
  return (
    <time dateTime={at} title={at} {...stylex.props(styles.when)}>
      {when(at)}
    </time>
  );
}

/** A sentence of the engine's. What it put between backticks is set as code. */
export function Sentence({ text }: { text: string }) {
  return (
    <p {...stylex.props(styles.text)}>
      {text.split("`").map((piece, index) => (index % 2 === 1 ? <Code key={index}>{piece}</Code> : piece))}
    </p>
  );
}

/** An ID in full, to read or to copy. */
export function Id({ value }: { value: string }) {
  return (
    <span {...stylex.props(styles.whole)}>
      <code data-part="id" {...stylex.props(styles.code)}>
        {value}
      </code>
      <CopyButton text={value} label="Copy ID" size="icon-xs" />
    </span>
  );
}

export function LinkTo({
  path,
  query,
  children,
}: {
  path: Array<string | number>;
  query?: Record<string, QueryValue>;
  children: ReactNode;
}) {
  return (
    <a href={href(path, query)} {...stylex.props(styles.link)}>
      {children}
    </a>
  );
}

/**
 * An identity where a screen holds only its ID: what it is called, when
 * that was read, with the end of its ID, since two may be called the same.
 */
export function Identity({ id, labels }: { id: string; labels: Map<string, string> }) {
  const label = labels.get(id);
  if (label === undefined) return <Ref kind="identity" id={id} />;
  return (
    <span data-part="identity" {...stylex.props(styles.named)}>
      <Ref kind="identity" id={id} label={label} />
      <span aria-hidden {...stylex.props(styles.tail)}>
        {tail(id)}
      </span>
    </span>
  );
}

/** A link that reads as the next thing to do. */
export function Onward({
  path,
  query,
  children,
}: {
  path: Array<string | number>;
  query?: Record<string, QueryValue>;
  children: ReactNode;
}) {
  return (
    <a href={href(path, query)} {...stylex.props(styles.link, styles.onward)}>
      {children}
      <ArrowRightIcon />
    </a>
  );
}

/** A sentence with links in it. `names` gives records a name to show in place of their ID. */
export function Parts({ parts, names }: { parts: Part[]; names?: Map<string, string> }) {
  return (
    <>
      {parts.map((part, index) =>
        typeof part === "string" ? (
          <span key={index}>{part}</span>
        ) : (
          <Ref key={index} kind={part.kind} id={part.id} label={names?.get(part.id)} />
        ),
      )}
    </>
  );
}

/** The most operations of one changeset that are shown before the reader asks for the rest. */
const MOST_OPS = 200;

export function Ops({ ops, names }: { ops: Op[]; names?: Map<string, string> }) {
  const [all, setAll] = useState(false);
  const shown = all ? ops : ops.slice(0, MOST_OPS);
  return (
    <>
      <ol data-part="ops" {...stylex.props(styles.ops)}>
        {shown.map((op, index) => (
          <li key={index} {...stylex.props(styles.op)}>
            <Parts parts={describeOp(op)} names={names} />
          </li>
        ))}
      </ol>
      {shown.length < ops.length ? (
        <div {...stylex.props(styles.more)}>
          <Button variant="outline" size="sm" onClick={() => setAll(true)}>
            Show all {ops.length} operations
          </Button>
        </div>
      ) : null}
    </>
  );
}

/** The members of a JSON object, one per row. Anything else on one line. */
export function Json({ value }: { value: unknown }) {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return <Code>{plain(value)}</Code>;
  }
  return <Fields rows={Object.entries(value).map(([name, member]) => [name, <Code>{plain(member)}</Code>])} />;
}

/** A control under the words that say what it is for. */
export function Labelled({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label title={hint} {...stylex.props(styles.labelled)}>
      <span {...stylex.props(styles.label)}>{label}</span>
      {children}
    </label>
  );
}

/** A box to tick, with the words that say what ticking it means. */
export function Flag({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label {...stylex.props(styles.flag)}>
      <Checkbox aria-label={label} checked={checked} onCheckedChange={(next) => onChange(next === true)} />
      <span aria-hidden>{label}</span>
    </label>
  );
}

export interface Choice {
  value: string;
  label?: string;
}

/** One of a few values, chosen from the browser's own list. */
export function Select({
  name,
  value,
  defaultValue,
  onChange,
  choices,
  any,
}: {
  name?: string;
  value?: string;
  defaultValue?: string;
  onChange?: (value: string) => void;
  choices: Choice[];
  /** What the choice of no value is called. Without it there is no such choice. */
  any?: string;
}) {
  return (
    <NativeSelect.Root
      name={name}
      value={value}
      defaultValue={defaultValue}
      onChange={onChange ? (event) => onChange(event.target.value) : undefined}
    >
      {any !== undefined ? <NativeSelect.Option value="">{any}</NativeSelect.Option> : null}
      {choices.map((choice) => (
        <NativeSelect.Option key={choice.value} value={choice.value}>
          {choice.label ?? choice.value.replaceAll("_", " ")}
        </NativeSelect.Option>
      ))}
    </NativeSelect.Root>
  );
}

/**
 * The filters of a list. Each change goes to the address, so that a list
 * as it was narrowed can be linked to.
 */
export function Filters({
  path,
  query,
  search,
  selects = [],
  texts = [],
  flags = [],
}: {
  path: string[];
  query: Record<string, string>;
  /** A box for words to look for, which is the first thing in the row. */
  search?: { name: string; label: string; placeholder: string };
  selects?: Array<{ name: string; label: string; choices: Choice[] }>;
  texts?: Array<{ name: string; label: string }>;
  flags?: Array<{ name: string; label: string }>;
}) {
  const go = (name: string, value: QueryValue) => {
    navigate(href(path, { ...query, [name]: value }));
  };
  const typed = [...(search ? [search] : []), ...texts];
  return (
    <form
      role="search"
      {...stylex.props(styles.filters)}
      onSubmit={(event) => {
        event.preventDefault();
        const entered = new FormData(event.currentTarget);
        const values: Record<string, QueryValue> = {};
        for (const { name } of typed) values[name] = String(entered.get(name) ?? "").trim();
        navigate(href(path, { ...query, ...values }));
      }}
    >
      {search ? (
        <Labelled label={search.label}>
          <Input
            name={search.name}
            type="search"
            defaultValue={query[search.name] ?? ""}
            placeholder={search.placeholder}
            spellCheck={false}
            autoComplete="off"
            style={styles.searchBox}
          />
        </Labelled>
      ) : null}
      {selects.map(({ name, label, choices }) => (
        <Labelled key={name} label={label}>
          <Select value={query[name] ?? ""} onChange={(value) => go(name, value)} choices={choices} any="any" />
        </Labelled>
      ))}
      {texts.map(({ name, label }) => (
        <Labelled key={name} label={label}>
          <Input name={name} defaultValue={query[name] ?? ""} spellCheck={false} autoComplete="off" />
        </Labelled>
      ))}
      {flags.map(({ name, label }) => (
        <Flag key={name} label={label} checked={query[name] === "true"} onChange={(checked) => go(name, checked)} />
      ))}
      {typed.length > 0 ? (
        <Button type="submit" variant="outline">
          {search && texts.length === 0 ? "Search" : "Apply"}
        </Button>
      ) : null}
    </form>
  );
}

/**
 * Keeps which records of a list the reader ticked, in the order they
 * were ticked. With `most`, ticking one more lets go of the earliest.
 */
export function useTicks(most?: number): {
  chosen: string[];
  tick: (id: string) => ReactNode;
} {
  const [chosen, setChosen] = useState<string[]>([]);
  const toggle = (id: string) =>
    setChosen((before) =>
      before.includes(id)
        ? before.filter((other) => other !== id)
        : [...before, id].slice(-(most ?? before.length + 1)),
    );
  return {
    chosen,
    tick: (id) => (
      <Checkbox aria-label={`Select record ${id}`} checked={chosen.includes(id)} onCheckedChange={() => toggle(id)} />
    ),
  };
}

/** The link that explains two ticked records, or what to tick to get it. */
export function ExplainTicked({ chosen }: { chosen: string[] }) {
  return chosen.length === 2 ? (
    <Onward path={["explain"]} query={{ a: chosen[0], b: chosen[1] }}>
      Explain selected
    </Onward>
  ) : (
    <Quiet>Select two to explain.</Quiet>
  );
}

/**
 * Keeps which two records of a list the reader ticked, and gives the
 * link that explains them.
 */
export function usePair(): {
  tick: (id: string) => ReactNode;
  explain: ReactNode;
} {
  const { chosen, tick } = useTicks(2);
  return { tick, explain: <ExplainTicked chosen={chosen} /> };
}

const styles = stylex.create({
  page: {
    gap: spacing["6"],
    display: "flex",
    flexDirection: "column",
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    marginInline: "auto",
    maxWidth: "76rem",
    paddingBlock: "1.75rem 4rem",
    paddingInline: { default: "2rem", "@media (width < 48rem)": "1rem" },
    width: "100%",
    boxSizing: "border-box",
  },
  pageHead: {
    gap: spacing["4"],
    alignItems: "flex-start",
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "space-between",
  },
  pageTitles: {
    gap: spacing["1.5"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  kind: {
    gap: spacing["2"],
    alignItems: "center",
    color: colors.mutedForeground,
    display: "flex",
    flexWrap: "wrap",
    fontSize: typography.fontSizeXs,
    fontWeight: typography.fontWeightMedium,
    lineHeight: typography.lineHeightXs,
  },
  h1: {
    margin: 0,
    fontSize: "1.5rem",
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.02em",
    lineHeight: "2rem",
    overflowWrap: "anywhere",
  },
  about: {
    color: colors.mutedForeground,
    maxWidth: "46rem",
  },
  actions: {
    gap: spacing["2"],
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
  },
  section: {
    gap: spacing["3"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  sectionHead: {
    gap: spacing["3"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "space-between",
  },
  sectionTitles: {
    gap: spacing["1"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  h2: {
    margin: 0,
    fontSize: typography.fontSizeBase,
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.01em",
    lineHeight: typography.lineHeightBase,
  },
  count: {
    color: colors.mutedForeground,
    fontVariantNumeric: "tabular-nums",
    fontWeight: typography.fontWeightRegular,
  },
  panel: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    backgroundColor: colors.background,
    overflow: "hidden",
  },
  panelPadded: {
    padding: spacing["4"],
  },
  empty: {
    padding: spacing["6"],
    backgroundColor: colors.card,
    color: colors.mutedForeground,
    textAlign: "center",
  },
  row: {
    gap: spacing["2"],
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
  },
  rowEnd: {
    justifyContent: "flex-end",
  },
  stack: {
    gap: spacing["3"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  quiet: {
    margin: 0,
    color: colors.mutedForeground,
  },
  reading: {
    gap: spacing["2"],
    alignItems: "center",
    display: "flex",
  },
  text: {
    margin: 0,
    maxWidth: "52rem",
  },
  reason: {
    color: colors.mutedForeground,
    fontStyle: "italic",
  },
  code: {
    borderRadius: radius.sm,
    paddingBlock: "0.0625rem",
    paddingInline: spacing["1"],
    backgroundColor: colors.muted,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
    overflowWrap: "anywhere",
  },
  lines: {
    margin: 0,
    padding: 0,
    gap: spacing["1"],
    display: "flex",
    flexDirection: "column",
    listStyleType: "none",
  },
  notice: {
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["2.5"],
    paddingBlock: spacing["2.5"],
    paddingInline: spacing["3"],
    alignItems: "flex-start",
    display: "flex",
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
  },
  noticeIcon: {
    flexShrink: 0,
    fontSize: "1rem",
    marginTop: "0.125rem",
  },
  noticeBody: {
    gap: spacing["1.5"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
    overflowWrap: "anywhere",
  },
  fields: {
    margin: 0,
    columnGap: spacing["6"],
    rowGap: spacing["2"],
    display: "grid",
    gridTemplateColumns: "minmax(7rem, max-content) minmax(0, 1fr)",
  },
  field: {
    display: "grid",
    gridColumn: "1 / -1",
    gridTemplateColumns: "subgrid",
  },
  term: {
    color: colors.mutedForeground,
  },
  value: {
    margin: 0,
    minWidth: 0,
    overflowWrap: "anywhere",
  },
  headRow: {
    backgroundColor: colors.card,
  },
  bodyRow: {
    backgroundColor: { default: "transparent", ":hover": colors.card },
    borderBottomWidth: { default: 1, ":last-child": 0 },
  },
  head: {
    paddingInline: spacing["3"],
    fontSize: typography.fontSizeXs,
    height: "2.25rem",
  },
  cell: {
    paddingBlock: spacing["2"],
    paddingInline: spacing["3"],
  },
  number: {
    fontVariantNumeric: "tabular-nums",
    textAlign: "end",
  },
  tick: {
    width: "1.5rem",
  },
  // A cell of running text wraps between words, and keeps the width to read
  // them in: on a narrow screen the table scrolls sideways before a name
  // is broken into letters.
  wrap: {
    minWidth: "10rem",
    overflowWrap: "break-word",
    whiteSpace: "normal",
  },
  more: {
    display: "flex",
    justifyContent: "center",
  },
  word: {
    borderRadius: radius.full,
    gap: spacing["1.5"],
    paddingBlock: "0.0625rem",
    paddingInline: spacing["2"],
    alignItems: "center",
    display: "inline-flex",
    fontSize: typography.fontSizeXs,
    fontWeight: typography.fontWeightMedium,
    lineHeight: typography.lineHeightXs,
    whiteSpace: "nowrap",
  },
  mark: {
    borderRadius: radius.full,
    flexShrink: 0,
    height: "0.375rem",
    width: "0.375rem",
  },
  source: {
    borderColor: colors.border,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["1.5"],
    paddingBlock: "0.0625rem",
    paddingInline: spacing["1.5"],
    alignItems: "center",
    backgroundColor: colors.background,
    display: "inline-flex",
    fontSize: typography.fontSizeXs,
    fontWeight: typography.fontWeightMedium,
    lineHeight: typography.lineHeightXs,
    whiteSpace: "nowrap",
  },
  sourceMark: {
    borderRadius: "0.125rem",
    flexShrink: 0,
    height: "0.5rem",
    width: "0.5rem",
  },
  sourceCount: {
    color: colors.mutedForeground,
    fontVariantNumeric: "tabular-nums",
    fontWeight: typography.fontWeightRegular,
  },
  sources: {
    gap: spacing["1"],
    display: "inline-flex",
    flexWrap: "wrap",
  },
  link: {
    color: tones.link,
    textDecorationLine: { default: "none", ":hover": "underline" },
    textUnderlineOffset: "0.15em",
  },
  onward: {
    gap: spacing["1"],
    alignItems: "center",
    display: "inline-flex",
    fontWeight: typography.fontWeightMedium,
  },
  id: {
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
  },
  named: {
    columnGap: spacing["1.5"],
    alignItems: "baseline",
    display: "inline-flex",
    flexWrap: "wrap",
  },
  tail: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: typography.fontSizeXs,
  },
  when: {
    fontVariantNumeric: "tabular-nums",
    whiteSpace: "nowrap",
  },
  whole: {
    gap: spacing["1"],
    alignItems: "center",
    display: "inline-flex",
    flexWrap: "wrap",
  },
  ops: {
    margin: 0,
    gap: spacing["1"],
    paddingInlineStart: "1.75rem",
    color: colors.mutedForeground,
    display: "flex",
    flexDirection: "column",
    fontVariantNumeric: "tabular-nums",
  },
  op: {
    color: colors.foreground,
    overflowWrap: "anywhere",
    paddingInlineStart: spacing["1"],
  },
  labelled: {
    gap: spacing["1.5"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  label: {
    color: colors.mutedForeground,
    fontSize: typography.fontSizeXs,
    fontWeight: typography.fontWeightMedium,
    lineHeight: typography.lineHeightXs,
  },
  flag: {
    gap: spacing["2"],
    alignItems: "center",
    cursor: "pointer",
    display: "inline-flex",
    minHeight: "2rem",
  },
  choose: {
    margin: 0,
    borderRadius: radius.md,
    gap: "0.125rem",
    padding: "0.1875rem",
    alignSelf: "flex-start",
    backgroundColor: colors.muted,
    display: "inline-flex",
    flexWrap: "wrap",
    listStyleType: "none",
  },
  choiceItem: {
    display: "flex",
  },
  choice: {
    borderRadius: radius.sm,
    gap: spacing["1.5"],
    paddingBlock: spacing["1"],
    paddingInline: spacing["2.5"],
    alignItems: "center",
    color: { default: colors.mutedForeground, ":hover": colors.foreground },
    display: "inline-flex",
    fontWeight: typography.fontWeightMedium,
    textDecorationLine: "none",
  },
  choiceCurrent: {
    backgroundColor: colors.background,
    boxShadow: "0 1px 2px 0 rgb(0 0 0 / 0.08)",
    color: colors.foreground,
  },
  filters: {
    gap: spacing["3"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
  },
  searchBox: {
    width: "20rem",
    maxWidth: "100%",
  },
});

const noticeTones = stylex.create({
  good: { borderColor: "transparent", backgroundColor: tones.goodSurface, color: tones.good },
  waits: { borderColor: "transparent", backgroundColor: tones.waitsSurface, color: tones.waits },
  bad: { borderColor: "transparent", backgroundColor: tones.badSurface, color: tones.bad },
  plain: { borderColor: colors.border, backgroundColor: colors.card, color: colors.foreground },
});

const wordTones = stylex.create({
  good: { backgroundColor: tones.goodSurface, color: tones.good },
  waits: { backgroundColor: tones.waitsSurface, color: tones.waits },
  bad: { backgroundColor: tones.badSurface, color: tones.bad },
  plain: { backgroundColor: colors.muted, color: colors.mutedForeground },
});

const markTones = stylex.create({
  good: { backgroundColor: tones.goodMark },
  waits: { backgroundColor: tones.waitsMark },
  bad: { backgroundColor: tones.badMark },
  plain: { backgroundColor: tones.plainMark },
});

const sourceMarks = stylex.create({
  source1: { backgroundColor: tones.source1 },
  source2: { backgroundColor: tones.source2 },
  source3: { backgroundColor: tones.source3 },
  source4: { backgroundColor: tones.source4 },
  source5: { backgroundColor: tones.source5 },
  source6: { backgroundColor: tones.source6 },
});
