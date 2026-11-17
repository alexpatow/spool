// The pieces the screens are made of.

import { useEffect, useState } from "react";
import type { ReactNode } from "react";

import { SpoolError } from "./api.ts";
import type { Op } from "./api.ts";
import { describeOp, plain, short, when } from "./format.ts";
import type { Part } from "./format.ts";
import type { Loaded, Pages } from "./hooks.ts";
import { href } from "./router.ts";
import type { QueryValue } from "./router.ts";

export function message(error: unknown): string {
  if (error instanceof SpoolError) return error.message;
  if (error instanceof Error) return `The server did not answer: ${error.message}`;
  return String(error);
}

export function Failure({ error }: { error: unknown }) {
  return (
    <p className="notice failure" role="alert">
      {message(error)}
    </p>
  );
}

/** What was read, or that it is being read, or why it could not be. */
export function Load<T>({ state, children }: { state: Loaded<T>; children: (data: T) => ReactNode }) {
  if (state.status === "loading") return <p className="quiet">Reading…</p>;
  if (state.status === "failed") return <Failure error={state.error} />;
  return <>{children(state.data)}</>;
}

export function Page({ title, about, children }: { title: string; about?: ReactNode; children: ReactNode }) {
  useEffect(() => {
    document.title = `${title} · Spool studio`;
  }, [title]);
  return (
    <section className="page">
      <header className="page-head">
        <h1>{title}</h1>
        {about ? <p className="quiet">{about}</p> : null}
      </header>
      {children}
    </section>
  );
}

export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="section">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

/** Label and value, one pair per row. A value that is `null` or `undefined` leaves its row out. */
export function Fields({ rows }: { rows: Array<[string, ReactNode]> }) {
  return (
    <dl className="fields">
      {rows
        .filter(([, value]) => value !== null && value !== undefined)
        .map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
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
}

export function Table<T>({
  columns,
  rows,
  rowKey,
  empty,
}: {
  columns: Array<Column<T>>;
  rows: T[];
  rowKey: (row: T) => string | number;
  /** What to say when there is nothing to list. */
  empty: string;
}) {
  if (rows.length === 0) return <p className="quiet">{empty}</p>;
  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column.title} className={column.number ? "number" : column.title === "" ? "tick" : undefined}>
                {column.title}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={rowKey(row)}>
              {columns.map((column) => (
                <td key={column.title} className={column.number ? "number" : undefined}>
                  {column.cell(row)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
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
}) {
  return (
    <Load state={pages.loaded}>
      {(rows) => (
        <>
          <Table rows={rows} {...table} />
          {pages.more ? (
            <p>
              <button type="button" onClick={pages.readMore} disabled={pages.reading}>
                {pages.reading ? "Reading…" : "Show more"}
              </button>
            </p>
          ) : null}
        </>
      )}
    </Load>
  );
}

const TONES: Record<string, string> = {
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
export function Word({ value, label }: { value: string; label?: string }) {
  return <span className={`word ${TONES[value] ?? "plain"}`}>{label ?? value.replaceAll("_", " ")}</span>;
}

export type Kind = "observation" | "identity" | "document" | "draft" | "run" | "review" | "log";

const SCREENS: Record<Kind, string> = {
  observation: "observations",
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
  return (
    <a className={typeof id === "string" ? "id" : undefined} href={href([SCREENS[kind], text])} title={text}>
      {label ?? short(text)}
    </a>
  );
}

/** A time to the second, where the reader is. The time as the server gave it shows on hover. */
export function When({ at }: { at: string }) {
  return (
    <time className="when" dateTime={at} title={at}>
      {when(at)}
    </time>
  );
}

/** A sentence of the engine's. What it put between backticks is set as code. */
export function Sentence({ text }: { text: string }) {
  return <p>{text.split("`").map((piece, index) => (index % 2 === 1 ? <code key={index}>{piece}</code> : piece))}</p>;
}

/** An ID in full, to read or to copy. */
export function Id({ value }: { value: string }) {
  return <code className="id whole">{value}</code>;
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
  return <a href={href(path, query)}>{children}</a>;
}

/** A sentence with links in it. `names` gives observations a name to show in place of their ID. */
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
      <ol className="ops">
        {shown.map((op, index) => (
          <li key={index}>
            <Parts parts={describeOp(op)} names={names} />
          </li>
        ))}
      </ol>
      {shown.length < ops.length ? (
        <p>
          <button type="button" onClick={() => setAll(true)}>
            Show all {ops.length} operations
          </button>
        </p>
      ) : null}
    </>
  );
}

/** The members of a JSON object, one per row. Anything else on one line. */
export function Json({ value }: { value: unknown }) {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return <code>{plain(value)}</code>;
  }
  return <Fields rows={Object.entries(value).map(([name, member]) => [name, <code>{plain(member)}</code>])} />;
}

export interface Choice {
  value: string;
  label?: string;
}

/**
 * The filters of a list. Each change goes to the address, so that a list
 * as it was narrowed can be linked to.
 */
export function Filters({
  path,
  query,
  selects = [],
  texts = [],
  flags = [],
}: {
  path: string[];
  query: Record<string, string>;
  selects?: Array<{ name: string; label: string; choices: Choice[] }>;
  texts?: Array<{ name: string; label: string }>;
  flags?: Array<{ name: string; label: string }>;
}) {
  const go = (name: string, value: QueryValue) => {
    window.location.hash = href(path, { ...query, [name]: value });
  };
  return (
    <form
      className="filters"
      onSubmit={(event) => {
        event.preventDefault();
        const entered = new FormData(event.currentTarget);
        const typed: Record<string, QueryValue> = {};
        for (const { name } of texts) typed[name] = String(entered.get(name) ?? "").trim();
        window.location.hash = href(path, { ...query, ...typed });
      }}
    >
      {selects.map(({ name, label, choices }) => (
        <label key={name}>
          {label}
          <select value={query[name] ?? ""} onChange={(event) => go(name, event.target.value)}>
            <option value="">any</option>
            {choices.map((choice) => (
              <option key={choice.value} value={choice.value}>
                {choice.label ?? choice.value.replaceAll("_", " ")}
              </option>
            ))}
          </select>
        </label>
      ))}
      {texts.map(({ name, label }) => (
        <label key={name}>
          {label}
          <input name={name} defaultValue={query[name] ?? ""} spellCheck={false} autoComplete="off" />
        </label>
      ))}
      {flags.map(({ name, label }) => (
        <label key={name} className="flag">
          <input
            type="checkbox"
            checked={query[name] === "true"}
            onChange={(event) => go(name, event.target.checked)}
          />
          {label}
        </label>
      ))}
      {texts.length > 0 ? <button type="submit">Apply</button> : null}
    </form>
  );
}

/**
 * Keeps which two observations of a list the reader ticked, and gives the
 * link that explains them.
 */
export function usePair(): {
  tick: (id: string) => ReactNode;
  explain: ReactNode;
} {
  const [chosen, setChosen] = useState<string[]>([]);
  const toggle = (id: string) =>
    setChosen((before) => (before.includes(id) ? before.filter((other) => other !== id) : [...before, id].slice(-2)));
  return {
    tick: (id) => (
      <input
        type="checkbox"
        aria-label={`Choose observation ${id} to explain`}
        checked={chosen.includes(id)}
        onChange={() => toggle(id)}
      />
    ),
    explain:
      chosen.length === 2 ? (
        <p className="notice">
          <LinkTo path={["explain"]} query={{ a: chosen[0], b: chosen[1] }}>
            Explain the two that are ticked
          </LinkTo>
        </p>
      ) : (
        <p className="quiet">Tick two observations to ask why they share an identity, or why they do not.</p>
      ),
  };
}
