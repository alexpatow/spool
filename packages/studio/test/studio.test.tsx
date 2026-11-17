// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.

import { afterAll, afterEach, beforeAll, describe, expect, spyOn, test } from "bun:test";

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import { TOKEN } from "../../client/test/server.ts";
import type { Server } from "../../client/test/server.ts";
import type { SpoolClient } from "../src/api.ts";
import { App, THEME_KEY, TOKEN_KEY } from "../src/app.tsx";
import { here } from "../src/router.ts";
import {
  SEEDING,
  calls,
  counted,
  field,
  follow,
  heading,
  idle,
  memory,
  open,
  parts,
  rowsOf,
  screens,
  start,
  stopAll,
  texts,
  wrapped,
} from "./harness.tsx";
import { REVIEWER, companies, news, row, seedCompanies, seedNews } from "./seed.ts";

afterEach(() => {
  cleanup();
  window.history.replaceState(null, "", "/");
});

afterAll(stopAll);

/** An identity where a screen read what it is called: its label, and the end of its ID. */
const CALLED = / \(…[0-9a-f]{4}\)$/;

/** What a screen that holds only the ID of an identity shows for it: its label, then the end of its ID. */
async function shownFor(client: SpoolClient, identity: string): Promise<string> {
  const { identities } = await client.identities({ ids: [identity] });
  return `${identities[0]!.label}${identity.slice(-4)}`;
}

describe("the door", () => {
  let server: Server;

  beforeAll(async () => {
    server = await start(companies, 11);
  }, SEEDING);

  test("asks for the token when the one of `spool dev` is refused, and keeps the one that works", async () => {
    const storage = open(server, "", memory());
    expect(await screen.findByText("SPOOL_TOKEN")).toBeTruthy();
    expect(screen.getByText(server.url)).toBeTruthy();
    expect(document.title).toBe("Spool studio");

    const token = screen.getByLabelText("Token") as HTMLInputElement;
    expect(token.type).toBe("password");
    fireEvent.change(token, { target: { value: "not-the-token" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText("Token rejected.")).toBeTruthy();

    fireEvent.change(screen.getByLabelText("Token"), { target: { value: TOKEN } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await heading("Overview");
    expect(storage.getItem(TOKEN_KEY)).toBe(TOKEN);

    fireEvent.click(screen.getByRole("button", { name: "Change token" }));
    expect(await screen.findByLabelText("Token")).toBeTruthy();
    expect(storage.getItem(TOKEN_KEY)).toBeNull();
  });

  test("opens without asking when the token is the one of `spool dev`", async () => {
    // A server whose token is `dev`, which is what `spool dev` starts.
    const sent: Array<string | null> = [];
    const dev = wrapped((input, init) => {
      const headers = new Headers(init?.headers);
      sent.push(headers.get("Authorization"));
      headers.set("Authorization", `Bearer ${TOKEN}`);
      return fetch(input, { ...init, headers });
    });
    render(<App url={server.url} fetch={dev} storage={memory()} />);
    await heading("Overview");
    await screen.findByText("Entity type company");
    expect(new Set(sent)).toEqual(new Set(["Bearer dev"]));
  });

  test("says so when the server does not answer, and asks again", async () => {
    let down = true;
    const flaky = wrapped((input, init) =>
      down ? Promise.reject(new TypeError("connection refused")) : fetch(input, init),
    );
    render(<App url={server.url} fetch={flaky} storage={memory(TOKEN)} />);
    expect((await screen.findByRole("alert")).textContent).toBe("The server did not answer: connection refused");
    down = false;
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await heading("Overview");
  });
});

describe("company records a reviewer corrected", () => {
  let server: Server;

  beforeAll(async () => {
    server = await start(companies, 7);
    await seedCompanies(server.client);
    calls.length = 0;
  }, SEEDING);

  test("the address of every screen is one the server answers with the studio", async () => {
    open(server, "");
    await heading("Overview");
    const page = await (await fetch(`${server.url}/`)).text();
    const screens = [...document.querySelectorAll("nav[aria-label=Screens] a")].map((link) =>
      link.getAttribute("href")!,
    );
    expect(screens.length).toBeGreaterThan(5);
    for (const address of screens) {
      const answered = await fetch(`${server.url}${address}`);
      expect([address, answered.status]).toEqual([address, 200]);
      expect(await answered.text()).toBe(page);
      // What a screen names comes after its own name.
      if (address !== "/") expect((await fetch(`${server.url}${address}/of/these`)).status).toBe(200);
    }
    calls.length = 0;
  });

  test("the overview counts what the store holds and what waits for someone", async () => {
    open(server, "");
    await heading("Overview");
    await screen.findByText("Entity type company");
    expect(parts("stat")).toEqual(["16identities", "39records", "5unassigned", "1open review item"]);
    // What waits is marked, and each number leads to the list it counts.
    expect(
      [...document.querySelectorAll("[data-part=stat][data-waits]")].map((stat) => stat.getAttribute("href")),
    ).toEqual(["/records?type=company&unassigned=true", "/review?type=company"]);
    expect(field("Head")).toBe("log position 7");
    expect(field("Declaration")).toBe("version 1");
    expect(document.title).toBe("Overview · Spool studio");

    // What waits for someone is said first, with the way to it.
    const drafts = await server.client.drafts({ status: "open" });
    const waiting = document.querySelector("[data-part=notice]")!;
    expect(waiting.getAttribute("data-tone")).toBe("waits");
    expect(waiting.textContent).toContain("1 open review item (company)");
    expect(waiting.textContent).toContain("1 draft to approve (company)");
    expect(
      within(waiting as HTMLElement)
        .getByRole("link", { name: "Review" })
        .getAttribute("href"),
    ).toBe("/review");
    expect(
      within(waiting as HTMLElement)
        .getByRole("link", { name: "Open" })
        .getAttribute("href"),
    ).toBe(`/drafts/${drafts[0]!.id}`);

    // The same numbers stand beside their screens in the list of them.
    const nav = screens();
    expect(nav.getByRole("link", { name: /^Review/ }).textContent).toBe("Review1");
    expect(nav.getByRole("link", { name: /^Drafts/ }).textContent).toBe("Drafts1");
    expect(nav.getByRole("link", { name: "Overview" }).getAttribute("aria-current")).toBe("page");
  });

  test("the overview says where the records of a type came from", async () => {
    open(server, "");
    await heading("Overview");
    const table = await screen.findByRole("table", { name: "company sources" });
    const { store } = await server.client.info();
    const sources = store.entity_types[0]!.sources;
    expect(sources.map((source) => source.source)).toEqual(["crm", "registry"]);
    expect(rowsOf(table).map((line) => line.slice(0, 3))).toEqual(
      sources.map((source) => [source.source, String(source.records), String(source.unassigned)]),
    );
    expect(sources.reduce((sum, source) => sum + source.records, 0)).toBe(39);
    expect(sources.reduce((sum, source) => sum + source.unassigned, 0)).toBe(5);
    // A source leads to its records, and to those of them that have no identity.
    expect(within(table).getByRole("link", { name: "registry" }).getAttribute("href")).toBe(
      "/records?type=company&source=registry",
    );
    expect(within(table).getByRole("link", { name: "4" }).getAttribute("href")).toBe(
      "/records?type=company&source=registry&unassigned=true",
    );

    const runs = rowsOf(screen.getByRole("table", { name: "Latest runs" }));
    expect(runs.map((line) => line[0])).toEqual(["Run 10", "Run 9", "Run 8", "Run 7", "Run 6"]);
  });

  test("a run that left a draft says what it made and where the draft is", async () => {
    open(server, "/runs");
    await heading("Runs");
    const table = await screen.findByRole("table");
    const lines = rowsOf(table);
    expect(lines).toHaveLength(10);
    expect(lines[0]?.slice(0, 3)).toEqual(["Run 10", "reconcile", "succeeded"]);
    expect(lines[1]?.slice(0, 4)).toEqual(["Run 9", "match", "drafted", "company, rules"]);

    follow(within(table).getByRole("link", { name: "Run 9" }));
    await heading("Run 9");
    await screen.findByText("Resolution");
    expect(field("Status")).toBe("drafted");
    expect(field("Examined")).toBe("5 records, 1 of them from earlier review items");
    expect(field("Assigned")).toBe("4 records, 1 identity created");
    expect(field("Unassigned")).toBe("1 record in 1 review item");
    expect(field("Draft")).toMatch(/^[0-9a-f]{8}…[0-9a-f]{4}$/);

    const drafts = await server.client.drafts({ status: "open" });
    const draft = document.querySelector("dd a[href^='/drafts/']")!;
    expect(draft.getAttribute("href")).toBe(`/drafts/${drafts[0]!.id}`);
    follow(draft);
    await heading("Draft");
    await counted("Operations", 5);
    expect(field("Status")).toBe("open");
    expect(field("Runs")).toBe("match run 9, reconcile run 10");
    expect(field("Changes")).toBe("4 records assigned, 1 identity created");
    // An operation names a record by its source and key once it is read.
    await screen.findByRole("link", { name: "registry:X0000016" });
    // And an identity by what its records call it, with the end of its ID. The one the draft
    // would make has no record yet, so it is named by its ID.
    const placed = () => texts("[data-part=ops] li").filter((text) => /^assign record registry:X\d+ to /.test(text));
    await waitFor(() => expect(placed().filter((text) => CALLED.test(text))).toHaveLength(3));
    expect(placed()).toHaveLength(4);
    expect(placed().filter((text) => / to 0\S+$/.test(text))).toHaveLength(1);
    expect(texts("[data-part=ops] li").filter((text) => /^create company identity 0\S+$/.test(text))).toHaveLength(1);
  });

  test("the runs can be narrowed, and the address says how", async () => {
    open(server, "/runs");
    await heading("Runs");
    await screen.findByRole("table");
    fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "ingest" } });
    expect(here()).toBe("/runs?kind=ingest");
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(4));
    expect(rowsOf(screen.getByRole("table")).every((line) => line[1] === "ingest")).toBe(true);
    expect((screen.getByLabelText("Kind") as HTMLSelectElement).value).toBe("ingest");
  });

  test("the identities are listed by what their records call them, with where the records came from", async () => {
    open(server, "/identities");
    await heading("Identities");
    const lines = rowsOf(await screen.findByRole("table"));
    expect(lines).toHaveLength(16);
    expect(lines.reduce((sum, line) => sum + Number(line[2]), 0)).toBe(34);
    expect(screen.getByText("16 identities")).toBeTruthy();

    const { identities } = await server.client.identities({ limit: 100 });
    expect(lines.map((line) => line[0]).sort()).toEqual(identities.map((identity) => identity.label!).sort());
    const kestrel = lines.find((line) => line[0] === "Kestrel Analytics");
    expect(kestrel?.slice(1, 4)).toEqual(["company", "4", "crm3registry1"]);
    // The sources of each say how many of its records each gave.
    for (const line of lines) {
      const counts = [...line[3]!.matchAll(/\d+/g)].map((found) => Number(found[0]));
      expect(counts.reduce((sum, value) => sum + value, 0)).toBe(Number(line[2]));
    }
  });

  test("the identities are found by words their records hold", async () => {
    open(server, "/identities");
    await heading("Identities");
    await screen.findByRole("table");
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: " halden energy " } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    expect(here()).toBe("/identities?q=halden+energy");
    await waitFor(() => expect(rowsOf(screen.getByRole("table")).map((line) => line[0])).toEqual(["Halden Energy"]));

    // A word of a value that is not the name finds it too, whatever its case.
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "X0000004" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() =>
      expect(rowsOf(screen.getByRole("table")).map((line) => line[0])).toEqual(["Kestrel Analytics"]),
    );

    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "nothing-holds-this" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => expect(parts("empty")).toEqual(["No identities match “nothing-holds-this”."]));
  });

  test("an identity shows its members and the constraints on them", async () => {
    const split = await row(server.client, "registry", "X0000010");
    open(server, `/identities/${split.identity}`);
    // It is called what its record calls it.
    await heading("PIONEER SUPPLIES LTD");
    await counted("Records", 1);
    expect(parts("id")).toEqual([split.identity!]);
    expect(document.title).toBe("PIONEER SUPPLIES LTD · Spool studio");
    // The constraint names a record that another identity holds.
    await screen.findByRole("link", { name: "crm:c-110" });
    const records = rowsOf(screen.getByRole("table", { name: "Records" }));
    // Each record says where it came from, and what put it here.
    expect(records.map((line) => line.slice(1, 4))).toEqual([["PIONEER SUPPLIES LTD", "registry", "X0000010"]]);
    expect(records[0]?.[4]).toMatch(/ by run 2$/);
    expect(records[0]?.[5]).toBe("log position 5 by user:studio-demo “the Bristol company is another company”");
    // Splitting it out said the two are not one company, so that no run joins them again.
    const constraints = rowsOf(screen.getAllByRole("table").at(-1)!);
    expect(constraints).toEqual([["cannot link", "crm:c-110", "registry:X0000010", "log position 5"]]);
    expect(texts("[data-part=history] > li")).toHaveLength(1);
    expect(texts("[data-part=history] > li")[0]).toContain("“the Bristol company is another company”");
    // The identities a changeset names are called what their records call them.
    await waitFor(() =>
      expect(texts("[data-part=history] > li")[0]).toMatch(
        /move record registry:X0000010 from Pioneer Supplies \(…[0-9a-f]{4}\) to PIONEER SUPPLIES LTD \(…[0-9a-f]{4}\)$/,
      ),
    );
  });

  test("an identity says what its records say, and which of them say it", async () => {
    const kestrel = await row(server.client, "crm", "c-115");
    open(server, `/identities/${kestrel.identity}`);
    await heading("Kestrel Analytics");
    await counted("Records", 4);
    expect(screen.getByText("4 records from 2 sources")).toBeTruthy();

    // A field to a group of rows: each value that is held, and the records that hold it.
    const table = screen.getByRole("table", { name: "Fields" });
    const said = (name: string) =>
      [...table.querySelectorAll(`tr[data-field=${name}]`)].map((line) =>
        [...line.querySelectorAll("td")].slice(-2).map((cell) => cell.textContent),
      );
    expect(said("city")).toEqual([["Bristol", "crmc-104registryX0000004"]]);
    expect(said("legal_name")).toEqual([["KESTREL ANALYTICS LIMITED", "registryX0000004"]]);
    // Where they disagree, every value is there, the one most records hold first.
    expect(said("name")).toEqual([
      ["Kestrel Analytics", "crmc-104"],
      ["Kestrel", "crmc-115"],
    ]);
    expect(said("website")).toEqual([
      ["https://kestrel.example", "crmc-104"],
      ["https://www.kestrel.example", "crmc-115"],
    ]);
    const differing = [...table.querySelectorAll("tr[data-field]")]
      .filter((line) => line.querySelector("[data-part=word]")?.textContent === "2 values")
      .map((line) => line.getAttribute("data-field"));
    expect(differing).toEqual(["id", "name", "website"]);
    // A value leads to the record that holds it.
    expect(within(table).getAllByRole("link", { name: "c-115" })[0]?.getAttribute("href")).toBe(
      `/records/${kestrel.id}`,
    );

    // The row crm:c-104 came twice, and what it said the first time is not what it says.
    expect(screen.getByText("1 superseded version not counted.")).toBeTruthy();
    const records = rowsOf(screen.getByRole("table", { name: "Records" }));
    expect(records.map((line) => `${line[2]}:${line[3]}`).sort()).toEqual([
      "crm:c-104",
      "crm:c-104",
      "crm:c-115",
      "registry:X0000004",
    ]);
    expect(records.filter((line) => line[1]?.endsWith("superseded"))).toHaveLength(1);
  });

  test("an identity shows every changeset that touched it", async () => {
    const atlas = await row(server.client, "crm", "c-109");
    open(server, `/identities/${atlas.identity}`);
    await heading("Atlas Trading");
    await screen.findByRole("heading", { name: "History" });
    await screen.findAllByRole("link", { name: "crm:c-109" });

    // It was made by the engine, merged away by the reviewer, and brought
    // back when the merge was undone.
    // Two identities are called Atlas Trading, and the end of each ID tells them apart.
    const merged = await row(server.client, "crm", "c-108");
    const here = `Atlas Trading (…${atlas.identity!.slice(-4)})`;
    const there = `Atlas Trading (…${merged.identity!.slice(-4)})`;
    expect(here).not.toBe(there);
    await waitFor(() => expect(texts("[data-part=history] > li")[0]).toContain(here));
    const history = texts("[data-part=history] > li");
    expect(history).toHaveLength(3);
    expect(history[0]).toMatch(/^Log position 1 \S+ \S+ by engine, drafted by reconcile run 4/);
    expect(history[0]).toContain(`assign record crm:c-109 to ${here}`);
    expect(history[1]).toMatch(/^Log position 2 \S+ \S+ by user:studio-demo “same name, taken for one company”merge /);
    expect(history[1]).toEndWith(`merge ${here} into ${there}`);
    expect(history[2]).toMatch(/^Log position 4 \S+ \S+ by user:studio-demo “they are two companies after all”/);
    expect(history[2]).toContain(`reinstate identity ${here}`);
    expect(history[2]).toContain(`move record crm:c-109 from ${there} to ${here}`);
    expect(document.querySelector("[data-part=history] a[href='/log/4']")?.textContent).toBe("Log position 4");
  });

  test("an identity that was merged away leads to the one that stands for it", async () => {
    const { changesets } = await server.client.changesets({ after: 5, limit: 1 });
    const merge = changesets[0]!.changeset.ops.find((op) => op.op === "merge");
    if (merge?.op !== "merge") throw new Error("log position 6 has no merge");
    open(server, `/identities/${merge.from}`);
    await heading(/./);
    await counted("Records", 3);
    expect(parts("id")).toEqual([merge.from]);
    const merged = document.querySelector("[data-part=notice]")!;
    expect(merged.textContent).toMatch(/^Merged into 0\S+\. Showing its records\.$/);
    expect(merged.querySelector("a[data-kind=identity]")?.getAttribute("href")).toBe(`/identities/${merge.into}`);
    // The reviewer said two of them are one company, and the identity says so.
    expect(rowsOf(screen.getAllByRole("table").at(-1)!)).toEqual([
      ["must link", "crm:c-111", "registry:X0000011", "log position 6"],
    ]);
  });

  test("an identity that is not there is said not to be", async () => {
    open(server, "/identities/00000000-0000-7000-8000-000000000000");
    await heading("Identity");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "identity 00000000-0000-7000-8000-000000000000 does not exist",
    );
  });

  test("the records no identity holds can be listed, and two of them explained", async () => {
    open(server, "/records");
    await heading("Records");
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("checkbox", { name: "Unassigned" }));
    expect(here()).toBe("/records?unassigned=true");
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(5));
    const lines = rowsOf(screen.getByRole("table"));
    // Each is called what it says it is, beside the source and key it came with.
    expect(lines.map((line) => `${line[2]}:${line[3]} ${line[1]}`).sort()).toEqual([
      "crm:c-117 Pioneer Supplies",
      "registry:X0000015 HALDEN ENERGY LIMITED",
      "registry:X0000016 TAMARIND SYSTEMS LTD",
      "registry:X0000017 WEXCOMBE PRINT LTD",
      "registry:X0612001 FENNMORE ENERGY LIMITED",
    ]);
    expect(lines.every((line) => line[5] === "unassigned")).toBe(true);

    const ticks = screen.getAllByRole("checkbox", { name: /^Select record/ });
    fireEvent.click(ticks[0]!);
    expect(screen.queryByRole("link", { name: "Explain selected" })).toBeNull();
    fireEvent.click(ticks[1]!);
    follow(screen.getByRole("link", { name: "Explain selected" }));
    await heading("Explain");
    expect((await screen.findByText("Not both assigned")).getAttribute("data-part")).toBe("word");
  });

  test("the records are found by words they hold", async () => {
    open(server, "/records");
    await heading("Records");
    await screen.findByRole("table");
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "KESTREL" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(here()).toBe("/records?q=KESTREL");
    const { records } = await server.client.records({ q: "kestrel" });
    expect(records).toHaveLength(4);
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(4));
    expect(
      rowsOf(screen.getByRole("table"))
        .map((line) => line[6])
        .sort(),
    ).toEqual(records.map((record) => texts(`a[title='${record.id}'][data-kind=record]`).at(-1)!).sort());
    // Each says which identity it belongs to by what that identity is called.
    const kestrel = await shownFor(server.client, records[0]!.identity!);
    expect(kestrel).toMatch(/^Kestrel Analytics[0-9a-f]{4}$/);
    await waitFor(() =>
      expect(rowsOf(screen.getByRole("table")).map((line) => line[5])).toEqual([kestrel, kestrel, kestrel, kestrel]),
    );
    expect(document.querySelector("[data-part=identity] a")!.getAttribute("href")).toBe(
      `/identities/${records[0]!.identity}`,
    );

    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "nothing-holds-this" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(parts("empty")).toEqual(["No records match “nothing-holds-this”."]));
  });

  test("the records can be narrowed to a source and a key", async () => {
    open(server, "/records");
    await heading("Records");
    await screen.findByRole("table");
    fireEvent.change(screen.getByLabelText("Source"), { target: { value: "crm" } });
    fireEvent.change(screen.getByLabelText("Key"), { target: { value: "c-104" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(here()).toBe("/records?source=crm&key=c-104");
    // The row came in both batches, with a change, so it has two versions.
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(2));
  });

  test("a record shows what was ingested and the review item it waits in", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    open(server, `/records/${waiting.id}`);
    await heading("Pioneer Supplies");
    await screen.findByText("Payload");
    expect(parts("id")).toEqual([waiting.id]);
    expect(field("Source")).toBe("crm");
    expect(field("Key")).toBe("c-117");
    expect(field("Identity")).toBe("unassigned");
    expect(field("Review")).toBe("item 1 possible merge open");
    for (const [name, value] of Object.entries(waiting.payload as Record<string, string>)) {
      expect(field(name)).toBe(value);
    }
    // Why it has no identity is said before anything else.
    expect(parts("notice")).toEqual(["Unassigned, pending review item 1."]);

    follow(screen.getByRole("link", { name: "item 1" }));
    await heading("Review item 1");
    await screen.findByRole("article", { name: "Review item 1" });
    expect(field("Kind")).toBe("possible merge");
    expect(field("Status")).toBe("open");
    expect(field("Run")).toBe("reconcile run 7");
  });

  test("a review item shows what it is decided from: the record, the identities it matched, and why", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const found = await server.client.reviewCase(1);
    open(server, "/review");
    await heading("Review");
    const card = within(await screen.findByRole("article", { name: "Review item 1" }));
    expect(card.getByRole("heading", { level: 2 }).textContent).toBe("Pioneer Supplies matched 2 identities");
    expect(card.getByRole("link", { name: "Item 1" }).getAttribute("href")).toBe("/review/1");

    // The record that waits and the identities it matched are side by side.
    const table = card.getByRole("table", { name: "Review item 1" });
    const columns = within(table)
      .getAllByRole("columnheader")
      .slice(1)
      .map((column) => column.textContent);
    expect(columns).toEqual([
      "Unassigned recordPioneer Suppliescrmcrm:c-117",
      "Matched identityPioneer Supplies2 recordscrm1registry1",
      "Matched identityPIONEER SUPPLIES LTD1 recordregistry1",
    ]);
    expect([...table.querySelectorAll("th a[data-kind=identity]")].map((link) => link.getAttribute("title"))).toEqual(
      found.candidates.map((candidate) => candidate.identity.id),
    );
    expect(found.item.identities.sort()).toEqual(found.candidates.map((candidate) => candidate.identity.id).sort());

    // A field to a row, and a value of an identity says whether the record holds the same.
    const line = (name: string) =>
      [...table.querySelectorAll(`tr[data-field=${name}] td`)].map((cell) => cell.textContent);
    const agreeing = (name: string) =>
      [...table.querySelectorAll(`tr[data-field=${name}] td`)].map(
        (cell) => cell.querySelector("[data-agrees]")?.getAttribute("data-agrees") ?? null,
      );
    expect(line("name")).toEqual(["Pioneer Supplies", "Pioneer Supplies", "empty"]);
    expect(agreeing("name")).toEqual([null, "true", null]);
    expect(line("id")).toEqual(["c-117", "c-110", "empty"]);
    expect(agreeing("id")).toEqual([null, "false", null]);
    expect(line("country")).toEqual(["GB", "GB", "GB"]);
    expect(agreeing("country")).toEqual([null, "true", "true"]);
    // What the record does not hold is not compared.
    expect(line("city")).toEqual(["empty", "Leeds", "Bristol"]);
    expect(agreeing("city")).toEqual([null, null, null]);

    // The run's verdicts on the record and each identity's records, with the rule behind each.
    const verdicts = [...table.querySelectorAll("tr")].find((found) => found.textContent?.startsWith("Judgements"))!;
    expect([...verdicts.querySelectorAll("td")].map((cell) => parts("word", cell))).toEqual([
      [],
      ["match", "match"],
      ["match"],
    ]);
    expect(verdicts.textContent).toContain("with registry:X0000010 by same_name_and_country");
    const other = found.candidates[1]!.members[0]!;
    expect(
      within(verdicts as HTMLElement)
        .getAllByRole("link", { name: "explain" })
        .at(-1)
        ?.getAttribute("href"),
    ).toBe(`/explain?a=${waiting.id}&b=${other.id}`);

    // A reviewer split the two identities before the record came, and each says so.
    const decided = table.querySelector("tr[data-part=decided]")!;
    expect(decided.querySelector("th")!.textContent).toBe("Constraints");
    expect([...decided.querySelectorAll("td")].map((cell) => parts("word", cell))).toEqual([
      [],
      ["cannot link"],
      ["cannot link"],
    ]);
    await waitFor(() => expect(decided.textContent).toContain("crm:c-110 and registry:X0000010, since log position 5"));

    // Under each column is the decision it stands for. Merging the two is not one: they are kept apart.
    expect(card.getAllByRole("button").map((button) => button.textContent)).toEqual([
      "New identity",
      "Assign to Pioneer Supplies",
      "Assign to PIONEER SUPPLIES LTD",
      "Dismiss",
    ]);
    expect(parts("apart", card.getByRole("table").closest("article")!)).toEqual([
      "Cannot be merged: a cannot_link constraint is between them.",
    ]);
  });

  test("the review items that are no longer open are a choice away", async () => {
    open(server, "/review");
    await heading("Review");
    await screen.findByRole("article", { name: "Review item 1" });
    const statuses = within(screen.getByRole("list", { name: "Status" }));
    expect(statuses.getAllByRole("link").map((link) => link.textContent)).toEqual([
      "Open1",
      "Resolved",
      "Dismissed",
      "Superseded",
      "All",
    ]);
    expect(statuses.getByRole("link", { name: /^Open/ }).getAttribute("aria-current")).toBe("true");

    follow(statuses.getByRole("link", { name: "Resolved" }));
    expect(here()).toBe("/review?status=resolved");
    await waitFor(() => expect(parts("empty")).toEqual(["No review items."]));
    follow(within(screen.getByRole("list", { name: "Status" })).getByRole("link", { name: "All" }));
    await screen.findByRole("article", { name: "Review item 1" });
  });

  test("a record is explained with another that is named by its source and key", async () => {
    const waiting = await row(server.client, "registry", "X0612001");
    const other = await row(server.client, "registry", "X0000009");
    open(server, `/records/${waiting.id}`);
    await heading("FENNMORE ENERGY LIMITED");
    const ask = await screen.findByLabelText("Compare with");
    fireEvent.change(ask, { target: { value: "registry:nothing" } });
    fireEvent.click(within(screen.getByRole("main")).getByRole("button", { name: "Explain" }));
    expect((await screen.findByRole("alert")).textContent).toBe("No record found for registry:nothing.");

    fireEvent.change(ask, { target: { value: "registry:X0000009" } });
    fireEvent.click(within(screen.getByRole("main")).getByRole("button", { name: "Explain" }));
    await waitFor(() => expect(here()).toBe(`/explain?a=${waiting.id}&b=${other.id}`));
    await screen.findByText("Not both assigned");
    await screen.findByText("No block rule holds, so a run would not compare this pair.");
  });

  test("two records a reviewer merged and then parted are explained by what happened", async () => {
    open(server, "/explain");
    await heading("Explain");
    fireEvent.click(within(screen.getByRole("main")).getByRole("button", { name: "Explain" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Enter two records.");

    fireEvent.change(screen.getByLabelText("Left record"), { target: { value: "crm:c-108" } });
    fireEvent.change(screen.getByLabelText("Right record"), { target: { value: "crm:c-109" } });
    fireEvent.click(within(screen.getByRole("main")).getByRole("button", { name: "Explain" }));

    await screen.findByText("Different identities");
    const a = await row(server.client, "crm", "c-108");
    const b = await row(server.client, "crm", "c-109");
    expect(here()).toBe(`/explain?a=${a.id}&b=${b.id}`);

    // The sentences are the engine's own, with what it put in backticks set as code.
    const explanation = await server.client.explain(a.id, b.id);
    expect(texts("[data-part=outcome] p")).toEqual(explanation.summary.map((sentence) => sentence.replaceAll("`", "")));
    expect(texts("[data-part=outcome] p").join(" ")).toContain("they are two companies after all");
    expect(texts("[data-part=outcome] p code")).toContain("crm:c-109");
    expect(texts("[data-part=outcome] p").join(" ")).not.toContain("`");
    expect(document.querySelector("[data-part=outcome]")?.getAttribute("data-outcome")).toBe("different_identities");

    const [features, rules, comparisons] = screen.getAllByRole("table").map(rowsOf);
    expect(features).toContainEqual(["name_key", "atlas trading", "atlas trading", ""]);
    expect(features).toContainEqual(["trading_key", "-", "-", ""]);
    // Only a value both have is marked as shared: two that are missing are not.
    expect(texts("tr[data-same] td:first-child")).toEqual(["name", "name_key"]);
    expect(document.querySelector("tr[data-same] [aria-label='Same']")).toBeTruthy();
    expect(rules).toContainEqual(["block", "same_name", "l.name_key = r.name_key", "holds"]);
    expect(rules).toContainEqual(["match", "same_name_and_country", expect.any(String), "does not hold"]);
    expect(rules).toContainEqual(["block", "same_domain", "l.domain = r.domain", "unknown: a feature is null"]);
    expect(screen.getByText("A run would compare this pair and make no judgement.")).toBeTruthy();
    expect(comparisons?.map((line) => [line[0], line[2], line[4], line[5]])).toEqual([
      ["Run 3", "rules", "same_name", "none"],
    ]);
  });

  test("two versions of one row are explained by what joins them", async () => {
    const { records } = await server.client.records({ source: "crm", key: "c-104" });
    expect(records).toHaveLength(2);
    open(server, `/explain?a=${records[0]!.id}&b=${records[1]!.id}`);
    await heading("Explain");
    await screen.findByText("Same identity");
    const sides = [...document.querySelectorAll("[data-part=side]")];
    expect(sides.map((side) => field("Record", side))).toEqual(["crm:c-104", "crm:c-104"]);
    const identity = await shownFor(server.client, records[0]!.identity!);
    await waitFor(() => expect(sides.map((side) => field("Identity", side))).toEqual([identity, identity]));
    expect((screen.getByLabelText("Left record") as HTMLInputElement).value).toBe(records[0]!.id);
  });

  test("a record that is not there is said not to be", async () => {
    const a = await row(server.client, "crm", "c-108");
    open(server, `/explain?a=${a.id}&b=00000000-0000-4000-8000-000000000000`);
    await heading("Explain");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "there is no record 00000000-0000-4000-8000-000000000000",
    );
  });

  test("the log lists every changeset, and one shows what it did", async () => {
    const { changesets } = await server.client.changesets({ after: 3, limit: 1 });
    open(server, "/log");
    await heading("Log");
    const table = await screen.findByRole("table");
    const lines = rowsOf(table);
    expect(lines.map((line) => line[0])).toEqual(["1", "2", "3", "4", "5", "6", "7"]);
    expect(lines[0]).toEqual(["1", "engine", "40", "reconcile run 4", "reconcile new records"]);
    expect(lines[3]).toEqual(["4", REVIEWER, "3", "-", "they are two companies after all"]);

    follow(within(table).getByRole("link", { name: "4" }));
    await heading("Log position 4");
    await counted("Operations", 3);
    expect(field("Actor")).toBe(REVIEWER);
    expect(field("Reason")).toBe("“they are two companies after all”");
    expect(field("Base")).toBe("log position 3");
    expect(field("Changeset")).toBe(changesets[0]!.changeset.id);
    expect(field("Content hash")).toBe(changesets[0]!.content_hash);
    await screen.findByRole("link", { name: "crm:c-109" });
    await waitFor(() =>
      expect(texts("[data-part=ops] li")[0]).toMatch(/^reinstate identity Atlas Trading \(…[0-9a-f]{4}\)$/),
    );
    expect(texts("[data-part=ops] li")[1]).toMatch(
      /^move record crm:c-109 from Atlas Trading \(…[0-9a-f]{4}\) to Atlas Trading \(…[0-9a-f]{4}\)$/,
    );
  });

  test("a log position past the end is said not to be there", async () => {
    open(server, "/log/99");
    await heading("Log position 99");
    expect((await screen.findByRole("alert")).textContent).toBe("No log position 99.");
  });

  /** Opens the box that finds things, and gives it. */
  async function finding(): Promise<HTMLInputElement> {
    if (screen.queryByRole("dialog") === null) {
      fireEvent.click(screen.getByRole("button", { name: /^Search/ }));
    }
    return (await screen.findByRole("combobox", {
      name: "Search",
    })) as HTMLInputElement;
  }

  test("an ID, or a source and key, typed into the box that finds things goes to what it names", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const drafts = await server.client.drafts({ status: "open" });
    open(server, "/runs");
    await heading("Runs");

    fireEvent.change(await finding(), { target: { value: " crm:c-111 " } });
    fireEvent.click(await screen.findByRole("option", { name: "Go to crm:c-111" }));
    await waitFor(() => expect(here()).toBe(`/records/${orchard.id}`));
    await heading("Orchard");
    // It went there, so the box is closed.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.change(await finding(), { target: { value: orchard.identity! } });
    fireEvent.click(await screen.findByRole("option", { name: `Go to ${orchard.identity}` }));
    await waitFor(() => expect(here()).toBe(`/identities/${orchard.identity}`));
    await screen.findByText("Constraints");

    fireEvent.change(await finding(), { target: { value: drafts[0]!.id } });
    fireEvent.click(await screen.findByRole("option", { name: `Go to ${drafts[0]!.id}` }));
    await waitFor(() => expect(here()).toBe(`/drafts/${drafts[0]!.id}`));
    await heading("Draft");

    fireEvent.change(await finding(), { target: { value: "crm:nothing" } });
    fireEvent.click(await screen.findByRole("option", { name: "Go to crm:nothing" }));
    expect((await screen.findByRole("alert")).textContent).toBe("crm:nothing not found.");
    expect(here()).toBe(`/drafts/${drafts[0]!.id}`);
  });

  test("words typed into the box that finds things list the identities and the records that hold them", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    open(server, "/runs");
    await heading("Runs");

    // Before anything is typed, it lists the screens.
    const box = await finding();
    const listed = () => screen.getAllByRole("option").map((option) => option.textContent);
    await waitFor(() => expect(listed()).toContain("Identities"));
    expect(listed()).toHaveLength(9);

    fireEvent.change(box, { target: { value: "Pioneer" } });
    await screen.findByRole("option", { name: /^PIONEER SUPPLIES LTD company · 1 record/ });
    await idle();
    expect(listed()).toEqual([
      "Pioneer Suppliescompany · 2 recordscrm1registry1",
      "PIONEER SUPPLIES LTDcompany · 1 recordregistry1",
      "Pioneer Suppliesc-110crm",
      "PIONEER SUPPLIES LTDX0000009registry",
      "Pioneer Suppliesc-117crm",
      "PIONEER SUPPLIES LTDX0000010registry",
    ]);

    // Every word has to be held, and a word that names a screen lists it too.
    fireEvent.change(box, { target: { value: "pioneer nothing-holds-this" } });
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("No results for “pioneer nothing-holds-this”."),
    );
    expect(screen.queryAllByRole("option")).toEqual([]);

    fireEvent.change(box, { target: { value: "pioneer c-117" } });
    fireEvent.click(await screen.findByRole("option", { name: /^Pioneer Supplies c-117/ }));
    await waitFor(() => expect(here()).toBe(`/records/${waiting.id}`));
    await heading("Pioneer Supplies");
  });

  test("the page can be made dark, and stays so", async () => {
    const names = memory();
    open(server, "", undefined, names);
    await heading("Overview");
    expect(document.documentElement.dataset.theme).toBe("light");
    fireEvent.click(screen.getByRole("button", { name: "Dark theme" }));
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
    expect(names.getItem(THEME_KEY)).toBe("dark");
    // Payload nothing with is a class on the page: its policy allows no style of its own.
    expect(document.querySelectorAll("style")).toHaveLength(0);
    cleanup();

    open(server, "", undefined, names);
    await heading("Overview");
    expect(document.documentElement.dataset.theme).toBe("dark");
    fireEvent.click(screen.getByRole("button", { name: "Light theme" }));
    expect(names.getItem(THEME_KEY)).toBe("light");
  });

  test("an address no screen has says so", async () => {
    for (const address of ["/nowhere", "/runs/9/more", "/explain/this"]) {
      open(server, address);
      await heading("Not found");
      expect(screen.getByText("No screen at this address.")).toBeTruthy();
      cleanup();
    }
  });

  test("looking at the screens changes nothing", () => {
    expect(calls.length).toBeGreaterThan(60);
    expect(calls.filter((call) => !call.startsWith("GET /v1/"))).toEqual([]);
  });

  test("what changed since a screen was read is shown when it is read again", async () => {
    open(server, "/review");
    await heading("Review");
    await screen.findByRole("article", { name: "Review item 1" });
    expect(screens().getByRole("link", { name: /^Review/ }).textContent).toBe("Review1");

    // Someone else settles the item while the screen is open.
    await server.client.dismissReview(1, { actor: REVIEWER, reason: "looked at it" });
    expect(screen.getByRole("article", { name: "Review item 1" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    // It no longer waits, so it leaves the list of those that do.
    await waitFor(() => expect(parts("empty")).toHaveLength(1));
    expect(parts("empty")[0]).toStartWith("No open review items.");
    await waitFor(() => expect(screens().getByRole("link", { name: /^Review/ }).textContent).toBe("Review"));

    follow(within(screen.getByRole("list", { name: "Status" })).getByRole("link", { name: "Dismissed" }));
    const card = within(await screen.findByRole("article", { name: "Review item 1" }));
    expect(card.getByText(/^Dismissed by/).textContent).toMatch(
      /^Dismissed by user:studio-demo at \S+ \S+ “looked at it”\.$/,
    );
    // Nothing is left to decide.
    expect(card.queryAllByRole("button")).toEqual([]);

    follow(card.getByRole("link", { name: "Item 1" }));
    await heading("Review item 1");
    await screen.findByRole("article", { name: "Review item 1" });
    expect(field("Status")).toBe("dismissed");
    expect(field("Dismissed by")).toMatch(/^user:studio-demo at \S+ \S+ “looked at it”$/);
  });
});

describe("news items an extractor read", () => {
  let server: Server;

  beforeAll(async () => {
    server = await start(news, 5);
    await seedNews(server.client);
  }, SEEDING);

  test("a document shows its text with each mention where it was found", async () => {
    open(server, "/documents");
    await heading("Documents");
    const table = await screen.findByRole("table");
    expect(rowsOf(table).length).toBeGreaterThan(5);
    follow(within(table).getByRole("link", { name: "news:09-falcon-watch.txt" }));

    await heading("news:09-falcon-watch.txt");
    await counted("Mentions", 3);
    const { documents } = await server.client.documents({ source: "news", key: "09-falcon-watch.txt" });
    const record = await server.client.document(documents[0]!.id);
    expect(parts("document")).toEqual([record.text!]);
    expect(parts("mention")).toEqual(["The Falcon Ltd", "Orchard Ltd", "NORTHWIND TRADERS LTD"]);
    expect(field("Extracted by")).toMatch(/^suffix_ts in run \d+ \(3 mentions\)$/);
    expect(rowsOf(screen.getByRole("table")).map((line) => line.slice(1, 4))).toEqual([
      ["The Falcon Ltd", "0 to 14", "company"],
      ["Orchard Ltd", "70 to 81", "company"],
      ["NORTHWIND TRADERS LTD", "122 to 143", "company"],
    ]);
    // Each mention says which company it was taken for.
    const { records: mentions } = await server.client.records({ document: record.id });
    const taken = new Map<number, string>();
    for (const mention of mentions) {
      taken.set((mention.evidence as { start: number }).start, await shownFor(server.client, mention.identity!));
    }
    await waitFor(() =>
      expect(rowsOf(screen.getByRole("table")).map((line) => line[4])).toEqual([
        taken.get(0)!,
        taken.get(70)!,
        taken.get(122)!,
      ]),
    );

    // A mention is a record: its screen shows where it was found.
    follow(document.querySelectorAll("[data-part=mention]")[1]!);
    await heading("Orchard Ltd");
    await screen.findByText("Mention");
    expect(texts("[data-part=excerpt] mark")).toEqual(["Orchard Ltd"]);
    expect(parts("excerpt")[0]).toContain("Its maker, Orchard Ltd, said more stock would arrive in March.");
    expect(field("extractor")).toBe("suffix_ts");
    expect(field("name")).toBe("Orchard Ltd");
    // Its record says which company it was taken for, as the list of mentions did.
    await waitFor(() => expect(field("Identity")).toBe(taken.get(70)!));

    // And leads back to the document, at the mention.
    const back = screen.getByRole("link", { name: "news:09-falcon-watch.txt" });
    expect(back.getAttribute("href")).toBe(`/documents/${record.id}?at=70`);
    follow(back);
    await heading("news:09-falcon-watch.txt");
    await counted("Mentions", 3);
    expect(texts("[data-part=mention][data-here]")).toEqual(["Orchard Ltd"]);
  });

  test("a document's text is taken apart once, however many mentions it has", async () => {
    const { documents } = await server.client.documents({ source: "news", key: "09-falcon-watch.txt" });
    const record = await server.client.document(documents[0]!.id);
    // Counting a text's characters reads all of it, so a long document with
    // many mentions must not be read once for each of them.
    const taken = spyOn(Array, "from");
    const apart = () => taken.mock.calls.filter(([value]) => value === record.text).length;
    try {
      open(server, `/documents/${record.id}`);
      await heading("news:09-falcon-watch.txt");
      await counted("Mentions", 3);
      expect(apart()).toBe(1);

      // Ticking a mention shows the screen again, with the same text.
      const ticks = screen.getAllByRole("checkbox", { name: /^Select record/ });
      fireEvent.click(ticks[0]!);
      fireEvent.click(ticks[1]!);
      await screen.findByRole("link", { name: "Explain selected" });
      expect(apart()).toBe(1);
    } finally {
      taken.mockRestore();
    }
  });

  test("the mentions of one company in several documents share an identity", async () => {
    const { identities } = await server.client.identities({ limit: 100 });
    const largest = identities.reduce((most, identity) => (identity.members > most.members ? identity : most));
    expect(largest.members).toBeGreaterThan(1);
    const view = await server.client.identity(largest.id);
    const first = await server.client.record(view.members[0]!.record);
    const found = first.evidence as { key: string; start: number; end: number };
    const name = `news:${found.key}[${found.start}-${found.end}]`;

    open(server, `/identities/${largest.id}`);
    await heading(largest.label!);
    await counted("Records", largest.members);
    // Each is named by its document and where in it the mention is, in the
    // history, and that is its key among the records.
    await screen.findByRole("link", { name });
    const members = rowsOf(screen.getByRole("table", { name: "Records" }));
    expect(members).toHaveLength(largest.members);
    expect(members.every((member) => /^\S+\.txt\[\d+-\d+\]$/.test(member[3]!))).toBe(true);
    expect(members.map((member) => member[3])).toContain(`${found.key}[${found.start}-${found.end}]`);
    expect(members.every((member) => member[2] === "news")).toBe(true);
  });

  test("the mentions of one document can be listed as records", async () => {
    const { documents } = await server.client.documents({ source: "news", key: "09-falcon-watch.txt" });
    open(server, `/records?document=${documents[0]!.id}`);
    await heading("Records");
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(3));
    expect(
      rowsOf(screen.getByRole("table"))
        .map((line) => `${line[2]}:${line[3]} ${line[1]}`)
        .sort(),
    ).toEqual([
      "news:09-falcon-watch.txt[0-14] The Falcon Ltd",
      "news:09-falcon-watch.txt[122-143] NORTHWIND TRADERS LTD",
      "news:09-falcon-watch.txt[70-81] Orchard Ltd",
    ]);
  });

  test("the overview says which extractors the project has, and nothing waits", async () => {
    open(server, "");
    await heading("Overview");
    await screen.findByText("Extractors");
    expect(
      screen
        .getAllByRole("table")
        .flatMap(rowsOf)
        .map((line) => line[0]),
    ).toContain("suffix_ts");
    expect(field("Documents")).toBe(String((await server.client.info()).store.documents));
    expect(parts("notice")).toEqual(["Nothing to review."]);
    const nav = screens();
    expect(nav.getByRole("link", { name: "Review" }).textContent).toBe("Review");
    expect(nav.getByRole("link", { name: "Drafts" }).textContent).toBe("Drafts");
  });

  test("a source and a key with a space in it, typed into the box that finds things, goes to what it names", async () => {
    await server.client.addDocuments("news", [{ key: "Annual Report.txt", text: "Halden Energy Ltd reported." }]);
    const { documents } = await server.client.documents({ source: "news", key: "Annual Report.txt" });
    open(server, "/runs");
    await heading("Runs");

    fireEvent.click(screen.getByRole("button", { name: /^Search/ }));
    const box = await screen.findByRole("combobox", { name: "Search" });
    fireEvent.change(box, { target: { value: "news:Annual Report.txt" } });
    fireEvent.click(await screen.findByRole("option", { name: "Go to news:Annual Report.txt" }));
    await waitFor(() => expect(here()).toBe(`/documents/${documents[0]!.id}`));
    await heading("news:Annual Report.txt");
  });
});
