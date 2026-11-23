// What a screen is laid out with: the page and its sections, what is said
// on them, and what was read or could not be.

import * as stylex from "@stylexjs/stylex";
import { useEffect } from "react";
import type { ReactNode } from "react";

import { SpoolError } from "../api.ts";
import { Spinner } from "spool-ui/components/spinner";
import { plain } from "../format.ts";
import type { Loaded } from "../queries.ts";
import { CircleAlertIcon, CircleCheckIcon, InfoIcon } from "../icons.tsx";
import { colors, display, linen, radius, spacing, typography } from "spool-ui/tokens.stylex";
import { tones } from "../styles/tones.stylex";
import { shared } from "./shared.ts";

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
  return <code {...stylex.props(shared.code)}>{children}</code>;
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
                <span data-part="count" {...stylex.props(shared.count)}>
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

/** A surface with a border, for what belongs together. */
export function Panel({ children, flush = false }: { children: ReactNode; flush?: boolean }) {
  return <div {...stylex.props(shared.panel, !flush && styles.panelPadded)}>{children}</div>;
}

/** Things in a row, which goes on to the next line when there is no room. */
export function Row({ children, end = false }: { children: ReactNode; end?: boolean }) {
  return <div {...stylex.props(styles.row, end && styles.rowEnd)}>{children}</div>;
}

/** Labels and their values, one `Field` per row. */
export function Fields({ children }: { children: ReactNode }) {
  return <dl {...stylex.props(styles.fields)}>{children}</dl>;
}

/** A row of `Fields`. A value that is `null` or `undefined` leaves the row out. */
export function Field({ label, children }: { label: string; children?: ReactNode }) {
  if (children === null || children === undefined) return null;
  return (
    <div {...stylex.props(styles.field)}>
      <dt {...stylex.props(styles.term)}>{label}</dt>
      <dd {...stylex.props(styles.value)}>{children}</dd>
    </div>
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

/** The members of a JSON object, one per row. Anything else on one line. */
export function Json({ value }: { value: unknown }) {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return <Code>{plain(value)}</Code>;
  }
  return (
    <Fields>
      {Object.entries(value).map(([name, member]) => (
        <Field key={name} label={name}>
          <Code>{plain(member)}</Code>
        </Field>
      ))}
    </Fields>
  );
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
    fontFamily: display.fontFamily,
    fontSize: "1.875rem",
    fontVariationSettings: '"opsz" 36',
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: "2.25rem",
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
    fontFamily: display.fontFamily,
    fontSize: "1.25rem",
    fontVariationSettings: '"opsz" 24',
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: typography.lineHeightLg,
  },
  panelPadded: {
    padding: spacing["4"],
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
});

const noticeTones = stylex.create({
  good: { borderColor: "transparent", backgroundColor: tones.goodSurface, color: tones.good },
  waits: { borderColor: "transparent", backgroundColor: tones.waitsSurface, color: tones.waits },
  bad: { borderColor: "transparent", backgroundColor: tones.badSurface, color: tones.bad },
  plain: { borderColor: colors.border, backgroundColor: colors.card, color: colors.foreground },
});
