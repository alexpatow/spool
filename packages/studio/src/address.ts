// How the address of a screen is written and read. Its path names the
// screen, by the route files under `routes/`, and its query holds the
// screen's filters. The server answers the path of any screen with the one
// page, so a link to a screen can be copied and opened.

/** What a link or a filter gives for one name of the query. */
export type QueryValue = string | number | boolean | null | undefined;

/** The query of an address as it is before a route has read it: text, or what a link was given. */
export type Raw = Record<string, unknown>;

/** The query of an address, each name with the text it has there. */
export function parseSearch(search: string): Record<string, string> {
  const query: Record<string, string> = {};
  for (const [name, value] of new URLSearchParams(search)) query[name] = value;
  return query;
}

/**
 * The query of an address, with its `?`. A value that is empty, `false`,
 * `null` or `undefined` is left out.
 */
export function stringifySearch(query: Raw): string {
  const search = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    if (value === undefined || value === null || value === "" || value === false) continue;
    search.set(name, String(value));
  }
  const text = search.toString();
  return text === "" ? "" : `?${text}`;
}

/** A filter that is text: a name, a key, an ID, one of the API's words. */
export function text(value: unknown): string | undefined {
  if (typeof value === "number") return String(value);
  return typeof value === "string" && value !== "" ? value : undefined;
}

/** A filter that is on or off. Off is left out of the address. */
export function flag(value: unknown): boolean | undefined {
  return value === true || value === "true" ? true : undefined;
}

/** A filter that is a number, such as how many to list. */
export function number(value: unknown): number | undefined {
  if (value === "" || (typeof value !== "string" && typeof value !== "number")) return undefined;
  const read = Number(value);
  return Number.isNaN(read) ? undefined : read;
}

export type Order = "asc" | "desc";

/** Which way a list is sorted. */
export function order(value: unknown): Order | undefined {
  return value === "asc" || value === "desc" ? value : undefined;
}

type Filter = (value: unknown) => unknown;

/** The filters of a route, each as its reader gives it, and each left out of an address that does not hold it. */
export type Filters<Shape extends Record<string, Filter>> = {
  [Name in keyof Shape]?: Exclude<ReturnType<Shape[Name]>, undefined>;
};

/**
 * Reads the filters a route has out of a query: `shape` names each and says
 * how it is read. They come in the order the address has them, so that an
 * address is written again as it was. One the address holds with a value
 * that cannot be read is `undefined`, which leaves it out. A name that is
 * not a filter of the route is not read, and stays in the address.
 */
export function filters<Shape extends Record<string, Filter>>(raw: Raw, shape: Shape): Filters<Shape> {
  const read: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(raw)) {
    const filter = shape[name];
    if (filter !== undefined) read[name] = filter(value);
  }
  return read as Filters<Shape>;
}

/** How a list read in pages is sorted, which every such list keeps in its address. */
export const sorted = { sort: text, order };
