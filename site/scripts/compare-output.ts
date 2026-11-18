/**
 * Compares what a command printed with what a page shows. Used by
 * `check-samples.ts`, and kept apart from it so that it can be tested without
 * a `spool` binary.
 */

/** IDs and hashes: what the pages show, and what this run printed for each. */
export interface Ids {
  shown: Map<string, string>;
  printed: Map<string, string>;
}

export const noIds = (): Ids => ({ shown: new Map(), printed: new Map() });

const ID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|\b[0-9a-f]{64}\b/g;
const TIME = /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z/g;

export function tidy(text: string): string[] {
  const lines = text.split("\n").map((line) => line.trimEnd());
  while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** Forgets the IDs a line added, when the lines after it did not match. */
function forget(added: string[], ids: Ids) {
  for (const a of added) {
    const b = ids.shown.get(a);
    ids.shown.delete(a);
    if (b !== undefined) ids.printed.delete(b);
  }
}

/**
 * Pairs a printed line with the line the page shows, and records which ID
 * stands for which as it goes, so that the next ID is held to the ones before
 * it, on this line and on the lines already paired. Gives the IDs it added, or
 * nothing when the lines differ or one ID would stand for two.
 */
function pair(expected: string, actual: string, ids: Ids): string[] | undefined {
  const shape = (line: string) => line.replace(TIME, "<time>").replace(ID, "<id>");
  if (shape(expected) !== shape(actual)) return undefined;
  const want = expected.match(ID) ?? [];
  const got = actual.match(ID) ?? [];
  if (want.length !== got.length) return undefined;
  const added: string[] = [];
  for (const [index, a] of want.entries()) {
    const b = got[index] ?? "";
    const stands = ids.shown.get(a);
    const stoodFor = ids.printed.get(b);
    if (stands === undefined && stoodFor === undefined) {
      ids.shown.set(a, b);
      ids.printed.set(b, a);
      added.push(a);
    } else if (stands !== b || stoodFor !== a) {
      forget(added, ids);
      return undefined;
    }
  }
  return added;
}

/**
 * A printed line as the pages should show it: an ID the pages already show under
 * another value keeps that value, so that `--update` leaves one ID for one thing
 * across every page.
 */
export function asShown(line: string, ids: Ids): string {
  return line.replace(ID, (id) => {
    const known = ids.printed.get(id);
    if (known !== undefined) return known;
    ids.shown.set(id, id);
    ids.printed.set(id, id);
    return id;
  });
}

/**
 * Compares what was printed with what the page shows. `...` on a line of its
 * own matches any lines. When the two agree, `ids` holds the IDs of every line
 * that paired; when they do not, it is as it was.
 */
export function sameOutput(expected: string[], actual: string[], ids: Ids, prefix = false): boolean {
  const match = (e: number, a: number): boolean => {
    if (e === expected.length) return prefix || a === actual.length;
    if (expected[e] === "...") {
      for (let skip = a; skip <= actual.length; skip++) if (match(e + 1, skip)) return true;
      return false;
    }
    if (a === actual.length) return false;
    const added = pair(expected[e] ?? "", actual[a] ?? "", ids);
    if (added === undefined) return false;
    if (match(e + 1, a + 1)) return true;
    forget(added, ids);
    return false;
  };
  return match(0, 0);
}
