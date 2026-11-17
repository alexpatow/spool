// Where the studio is, kept in the part of the address after `#`. The server
// never sees it, so it has one page to serve and a link to a screen can be
// copied.

export interface Route {
  /** `["identities", "<id>"]` for `#/identities/<id>`. */
  path: string[];
  query: Record<string, string>;
}

export function parseHash(hash: string): Route {
  const [path = "", search = ""] = hash.replace(/^#/, "").split("?", 2);
  const query: Record<string, string> = {};
  for (const [name, value] of new URLSearchParams(search)) query[name] = value;
  return {
    path: path
      .split("/")
      .filter((part) => part !== "")
      .map((part) => decodeURIComponent(part)),
    query,
  };
}

export type QueryValue = string | number | boolean | null | undefined;

/**
 * The address of a screen. Parts of the path are encoded. A query value that
 * is empty, `false`, `null` or `undefined` is left out.
 */
export function href(path: Array<string | number>, query: Record<string, QueryValue> = {}): string {
  const search = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    if (value === undefined || value === null || value === "" || value === false) continue;
    search.set(name, String(value));
  }
  const text = search.toString();
  return `#/${path.map((part) => encodeURIComponent(String(part))).join("/")}${text === "" ? "" : `?${text}`}`;
}
