// Where the studio is: the path of the address names the screen, and the
// query its filters. The server answers the path of any screen with the one
// page, so a link to a screen can be copied and opened.

export interface Route {
  /** `["identities", "<id>"]` for `/identities/<id>`. */
  path: string[];
  query: Record<string, string>;
}

export function parseAddress(address: string): Route {
  const [path = "", search = ""] = address.split("#", 1)[0]!.split("?", 2);
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
  return `/${path.map((part) => encodeURIComponent(String(part))).join("/")}${text === "" ? "" : `?${text}`}`;
}

/** The address the browser is at, as `href` writes one. */
export function here(): string {
  return window.location.pathname + window.location.search;
}

const MOVED = "spool:moved";

/** Goes to a screen without loading the page again. */
export function navigate(address: string): void {
  if (address !== here()) window.history.pushState(null, "", address);
  window.dispatchEvent(new Event(MOVED));
}

/** Calls back when the address changes, by `navigate` or by the browser's back and forward. */
export function onMove(moved: () => void): () => void {
  window.addEventListener(MOVED, moved);
  window.addEventListener("popstate", moved);
  return () => {
    window.removeEventListener(MOVED, moved);
    window.removeEventListener("popstate", moved);
  };
}

/**
 * Whether a click on a link is one the studio answers itself: a plain click
 * on a link to one of its own screens. Anything else is the browser's, such
 * as a click that opens a new tab.
 */
export function ownLink(event: MouseEvent): string | null {
  if (event.defaultPrevented || event.button !== 0) return null;
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return null;
  const link = (event.target as Element | null)?.closest?.("a[href]");
  if (!link || link.hasAttribute("download") || (link.getAttribute("target") ?? "_self") !== "_self") return null;
  const address = link.getAttribute("href")!;
  return address.startsWith("/") && !address.startsWith("//") ? address : null;
}
