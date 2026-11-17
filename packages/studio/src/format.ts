// How the studio words what the server answers. Nothing here decides
// anything: a sentence that says why comes from the engine.

import type { ObservationRecord, Op } from "./api.ts";

/** `3 observations`, `1 identity`. */
export function count(number: number, what: string): string {
  if (number === 1) return `1 ${what}`;
  return `${number} ${what.endsWith("y") ? `${what.slice(0, -1)}ies` : `${what}s`}`;
}

/** A timestamp to the second, in the reader's time zone unless one is named. */
export function when(timestamp: string, timeZone?: string): string {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return timestamp;
  const parts = new Intl.DateTimeFormat("en-GB", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
    timeZone,
  }).formatToParts(date);
  const part = (type: string) => parts.find((found) => found.type === type)?.value ?? "";
  return `${part("year")}-${part("month")}-${part("day")} ${part("hour")}:${part("minute")}:${part("second")}`;
}

/**
 * An ID cut down to what tells it from the others on a page: its start and
 * its end. Identities minted together begin alike, since their IDs begin
 * with the time, and end differently.
 */
export function short(id: string): string {
  return id.length > 13 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
}

export function named(source: string, key?: string | null): string {
  return key === undefined || key === null ? `${source} (no key)` : `${source}:${key}`;
}

/** Where an extractor found a mention, if the evidence of an observation says. */
export interface Mention {
  document: string;
  /** The document's key within its source. */
  key: string;
  /** In Unicode code points. */
  start: number;
  end: number;
}

export function mentionOf(observation: Pick<ObservationRecord, "evidence">): Mention | null {
  const evidence = observation.evidence;
  if (typeof evidence !== "object" || evidence === null) return null;
  const { document, key, start, end } = evidence as Record<string, unknown>;
  if (typeof document !== "string" || typeof key !== "string") return null;
  if (!Number.isInteger(start) || !Number.isInteger(end)) return null;
  return { document, key, start: start as number, end: end as number };
}

/**
 * What to call an observation where a person reads it: a mention by its
 * document and span, a row by its source and key. The engine names them the
 * same way in the sentences of an explanation.
 */
export function observationName(observation: Pick<ObservationRecord, "id" | "source" | "key" | "evidence">): string {
  const mention = mentionOf(observation);
  if (mention !== null) return `${observation.source}:${mention.key}[${mention.start}-${mention.end}]`;
  if (observation.key !== undefined && observation.key !== null) return `${observation.source}:${observation.key}`;
  return observation.id;
}

/** A JSON value on one line, text without its quotes. */
export function plain(value: unknown): string {
  if (value === null || value === undefined) return "-";
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

/** What kind of thing an ID in a sentence names, so that it can be a link. */
export type Named = { kind: "observation" | "identity"; id: string };
export type Part = string | Named;

const observation = (id: string): Named => ({ kind: "observation", id });
const identity = (id: string): Named => ({ kind: "identity", id });

/** A changeset operation in words. */
export function describeOp(op: Op): Part[] {
  switch (op.op) {
    case "create_identity":
      return [`create ${op.entity_type} identity `, identity(op.identity)];
    case "assign": {
      const { from, to } = op;
      if (from && to)
        return ["move observation ", observation(op.observation), " from ", identity(from), " to ", identity(to)];
      if (to) return ["place observation ", observation(op.observation), " in ", identity(to)];
      if (from) return ["take observation ", observation(op.observation), " out of ", identity(from)];
      return ["leave observation ", observation(op.observation), " without an identity"];
    }
    case "merge":
      return ["merge ", identity(op.from), " into ", identity(op.into)];
    case "constrain":
      return ["constrain ", observation(op.a), " and ", observation(op.b), `: ${op.kind}`];
    case "unconstrain":
      return [`remove the ${op.kind} constraint on `, observation(op.a), " and ", observation(op.b)];
    case "reinstate":
      return ["reinstate identity ", identity(op.identity)];
  }
}

/** The observations a list of operations names, each once. */
export function observationsOf(ops: Op[]): string[] {
  const ids = ops
    .flatMap(describeOp)
    .flatMap((part) => (typeof part !== "string" && part.kind === "observation" ? [part.id] : []));
  return [...new Set(ids)];
}

/** The identities a list of operations names, each once. */
export function identitiesOf(ops: Op[]): string[] {
  const ids = ops
    .flatMap(describeOp)
    .flatMap((part) => (typeof part !== "string" && part.kind === "identity" ? [part.id] : []));
  return [...new Set(ids)];
}

/** The end of an ID, which tells two things apart that are called the same. */
export function tail(id: string): string {
  return id.slice(-4);
}

/** The text around a span of a document, on one line. */
export interface Excerpt {
  before: string;
  text: string;
  after: string;
  /** Whether the document goes on before `before` and after `after`. */
  cutBefore: boolean;
  cutAfter: boolean;
}

/**
 * A text as its code points, which is what the engine counts a span in.
 * Making it reads the whole text, so a screen makes it once for a document
 * and not once for each of its mentions.
 */
export function codePoints(text: string): string[] {
  return Array.from(text);
}

/** `start` and `end` count code points, as the engine does. `null` if the span is not in the text. */
export function excerpt(text: string, start: number, end: number, context = 80): Excerpt | null {
  const points = codePoints(text);
  if (start > end || end > points.length) return null;
  const from = Math.max(0, start - context);
  const to = Math.min(points.length, end + context);
  const line = (slice: string[]) => slice.join("").replace(/[\n\r\t]/g, " ");
  return {
    before: line(points.slice(from, start)),
    text: line(points.slice(start, end)),
    after: line(points.slice(end, to)),
    cutBefore: from > 0,
    cutAfter: to < points.length,
  };
}

export interface Span<T> {
  start: number;
  end: number;
  value: T;
}

/** A stretch of a document, and the spans that cover it. */
export interface Segment<T> {
  text: string;
  start: number;
  covered: T[];
}

/**
 * Cuts a text where a span begins or ends, so that each piece is covered by
 * the same spans from its first code point to its last. Spans may overlap.
 * One that is not in the text is left out. The text may be given as its
 * code points, by a caller that has them already.
 */
export function segments<T>(text: string | string[], spans: Array<Span<T>>): Array<Segment<T>> {
  const points = typeof text === "string" ? codePoints(text) : text;
  const inside = spans.filter((span) => span.start >= 0 && span.start < span.end && span.end <= points.length);
  const cuts = [...new Set([0, points.length, ...inside.flatMap((span) => [span.start, span.end])])].sort(
    (a, b) => a - b,
  );
  const pieces: Array<Segment<T>> = [];
  for (let index = 0; index + 1 < cuts.length; index++) {
    const start = cuts[index]!;
    const end = cuts[index + 1]!;
    pieces.push({
      text: points.slice(start, end).join(""),
      start,
      covered: inside.filter((span) => span.start <= start && end <= span.end).map((span) => span.value),
    });
  }
  return pieces;
}
