// How the address of a screen is written and read: by the router, with the
// rules of `src/address.ts` for the query.

import { describe, expect, test } from "bun:test";

import { parseSearch, stringifySearch } from "../src/address.ts";
import { createStudioRouter } from "../src/app.tsx";

const router = createStudioRouter();

/** The screen an address names: its route, what the path names, and its filters as the route reads them. */
function read(address: string) {
  const [path = "", query = ""] = address.split("?", 2);
  const matches = router.matchRoutes(path, parseSearch(query));
  const shown = matches.at(-1)!;
  return { route: shown.routeId, params: shown.params, search: shown.search };
}

describe("an address", () => {
  test("names a screen and what narrows it", () => {
    expect(read("/")).toEqual({ route: "/", params: {}, search: {} });
    expect(read("/runs/9")).toEqual({ route: "/runs/$id", params: { id: "9" }, search: {} });
    expect(read("/records?source=crm&key=c%2F1&unassigned=true")).toEqual({
      route: "/records/",
      params: {},
      search: { source: "crm", key: "c/1", unassigned: true },
    });
    // A filter is read as what it is, and one that cannot be is as if it were not there.
    expect(read("/runs?limit=3&order=desc&kind=load").search).toEqual({ limit: 3, order: "desc", kind: "load" });
    expect(read("/runs?limit=many&order=down").search).toEqual({ limit: undefined, order: undefined });
    expect(read("/documents/a%2Fb?at=70")).toEqual({ route: "/documents/$id", params: { id: "a/b" }, search: { at: 70 } });
    // What is not a filter of the screen is left as the address has it.
    expect(read("/people?page=1").search).toEqual({ page: "1" });
  });

  test("is written so that it reads back the same", () => {
    expect(router.buildLocation({ to: "/" }).href).toBe("/");
    expect(router.buildLocation({ to: "/runs/$id", params: { id: "9" } }).href).toBe("/runs/9");
    const address = router.buildLocation({
      to: "/records",
      search: { source: "crm", key: "a b&c", unassigned: true, type: "", document: undefined },
    }).href;
    expect(address).toBe("/records?source=crm&key=a+b%26c&unassigned=true");
    expect(read(address)).toEqual({
      route: "/records/",
      params: {},
      search: { source: "crm", key: "a b&c", unassigned: true },
    });
    const document = router.buildLocation({ to: "/documents/$id", params: { id: "a/b" } }).href;
    expect(document).toBe("/documents/a%2Fb");
    expect(read(document).params).toEqual({ id: "a/b" });
  });

  test("leaves out a filter that is empty or off", () => {
    expect(stringifySearch({})).toBe("");
    expect(stringifySearch({ q: "", unassigned: false, type: null, source: undefined })).toBe("");
    expect(stringifySearch({ q: "halden energy", limit: 0, unassigned: true })).toBe("?q=halden+energy&limit=0&unassigned=true");
    expect(parseSearch("?q=halden+energy&limit=0")).toEqual({ q: "halden energy", limit: "0" });
  });
});
