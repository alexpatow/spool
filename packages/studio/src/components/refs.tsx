// What names a thing: a link to its screen, what it is called, when it
// happened, and the API's own words for what it is.

import * as stylex from "@stylexjs/stylex";
import { createLink } from "@tanstack/react-router";
import type { LinkComponent, LinkProps } from "@tanstack/react-router";
import { forwardRef, useState } from "react";
import type { ComponentProps, ReactNode } from "react";

import type { Op } from "../api.ts";
import { Button } from "spool-ui/components/button";
import { CopyButton } from "spool-ui/components/copy-button";
import { describeOp, short, tail, when } from "../format.ts";
import type { Part } from "../format.ts";
import { ArrowRightIcon } from "../icons.tsx";
import { colors, linen, radius, spacing, typography } from "spool-ui/tokens.stylex";
import { tones } from "../styles/tones.stylex";
import { shared } from "./shared.ts";
import type { Tone } from "./layout.tsx";

/** Where a link leads: a screen, what it names, and its filters. */
export type Leads = Pick<LinkProps, "to" | "params" | "search">;

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
  taken_back: "plain",
  possible_merge: "waits",
  possible_split: "waits",
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

/** The screen of one thing of each kind. */
export const SCREENS = {
  record: "/records/$id",
  identity: "/identities/$id",
  document: "/documents/$id",
  draft: "/drafts/$id",
  run: "/runs/$id",
  review: "/review/$id",
  log: "/log/$id",
} as const satisfies Record<Kind, string>;

/**
 * The anchor of a link to a screen. The router marks every link to where the
 * reader is as the current one. The studio says which is current only where
 * a list has one, the screens and the choices, so the router's mark is left
 * out and the list's own is `current`.
 */
const Anchor = forwardRef<HTMLAnchorElement, ComponentProps<"a"> & { current?: "page" | "true" }>(
  function Anchor({ current, "aria-current": _routers, ...props }, ref) {
    const { "data-status": _status, disabled: _disabled, ...anchor } = props as typeof props & Record<string, unknown>;
    return <a ref={ref} {...anchor} aria-current={current} />;
  },
);

const RouterLink = createLink(Anchor);

const NOT_MARKED = {};

/**
 * A link to a screen of the studio, which goes there without loading the
 * page again: the router's link, with nothing added to how it looks.
 */
export const ScreenLink: LinkComponent<typeof Anchor> = (props) => <RouterLink activeProps={NOT_MARKED} {...props} />;

/** A link to the screen of one thing. A long ID is shortened and shown whole on hover. */
export function Ref({ kind, id, label }: { kind: Kind; id: string | number; label?: ReactNode }) {
  const text = String(id);
  const bare = label === undefined || label === null;
  return (
    <ScreenLink
      to={SCREENS[kind]}
      params={{ id: text }}
      title={text}
      data-kind={kind}
      {...stylex.props(
        styles.link,
        bare && (kind === "identity" || kind === "log") && styles.thread,
        bare && typeof id === "string" && styles.id,
      )}
    >
      {bare ? short(text) : label}
    </ScreenLink>
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

/** An ID in full, to read or to copy. */
export function Id({ value }: { value: string }) {
  return (
    <span {...stylex.props(styles.whole)}>
      <code data-part="id" {...stylex.props(shared.code)}>
        {value}
      </code>
      <CopyButton text={value} label="Copy ID" size="icon-xs" />
    </span>
  );
}

/** A link to a screen, in the words of the page. */
export const LinkTo: LinkComponent<typeof Anchor> = (props) => <ScreenLink {...props} {...stylex.props(styles.link)} />;

/**
 * Where a record is: its identity, or that it waits for one, or that its
 * source took it back and it waits for none.
 */
export function Placed({
  record,
  labels,
}: {
  record: { identity?: string | null; retracted?: number | null };
  labels: Map<string, string>;
}) {
  if (record.identity) return <Identity id={record.identity} labels={labels} />;
  return <Word value={record.retracted == null ? "unassigned" : "taken_back"} />;
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

const ArrowAnchor = forwardRef<HTMLAnchorElement, ComponentProps<typeof Anchor>>(function ArrowAnchor(
  { children, ...props },
  ref,
) {
  return (
    <Anchor ref={ref} {...props}>
      {children}
      <ArrowRightIcon />
    </Anchor>
  );
});

const ArrowLink = createLink(ArrowAnchor);

/** A link that reads as the next thing to do. */
export const Onward: LinkComponent<typeof ArrowAnchor> = (props) => (
  <ArrowLink activeProps={NOT_MARKED} {...props} {...stylex.props(styles.link, styles.onward)} />
);

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
        <div {...stylex.props(shared.more)}>
          <Button variant="outline" size="sm" onClick={() => setAll(true)}>
            Show all {ops.length} operations
          </Button>
        </div>
      ) : null}
    </>
  );
}

const styles = stylex.create({
  word: {
    borderRadius: radius.sm,
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
    color: colors.foreground,
    textDecorationColor: { default: colors.ring, ":hover": colors.foreground },
    textDecorationLine: "underline",
    textDecorationThickness: "1px",
    textUnderlineOffset: "0.2em",
  },
  // The site's one colour is for identities and the log: here their IDs.
  thread: {
    color: linen.thread,
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
