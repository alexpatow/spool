// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.

import { afterAll, afterEach, beforeAll, describe, expect, spyOn, test } from "bun:test";
import { rmSync } from "node:fs";

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import { TOKEN, serve } from "../../client/test/server.ts";
import type { Server } from "../../client/test/server.ts";
import { App, TOKEN_KEY } from "../src/app.tsx";
import { REVIEWER, companies, copyProject, news, row, seedCompanies, seedNews } from "./seed.ts";

const SEEDING = 120_000;

type Fetch = typeof fetch;

/** A `fetch` that does something before the real one is called. */
function wrapped(call: (...args: Parameters<Fetch>) => ReturnType<Fetch>): Fetch {
  return Object.assign(call, { preconnect: fetch.preconnect });
}

/** What a tab keeps, without a tab. */
function memory(token?: string) {
  const kept = new Map<string, string>(token === undefined ? [] : [[TOKEN_KEY, token]]);
  return {
    getItem: (name: string) => kept.get(name) ?? null,
    setItem: (name: string, value: string) => void kept.set(name, value),
    removeItem: (name: string) => void kept.delete(name),
  };
}

/** Every call the studio made, as `GET /v1/info`. */
const calls: string[] = [];
const recording = wrapped((input, init) => {
  calls.push(`${init?.method ?? "GET"} ${new URL(String(input)).pathname}`);
  return fetch(input, init);
});

/** Opens the studio at an address, with the token already entered unless `storage` says otherwise. */
function open(server: Server, address: string, storage = memory(TOKEN)) {
  window.location.hash = address;
  render(<App url={server.url} fetch={recording} storage={storage} />);
  return storage;
}

/** Follows a link of the studio, as a click in a browser would. */
function follow(link: Element) {
  const address = link.getAttribute("href");
  if (address === null || !address.startsWith("#")) throw new Error(`${link.outerHTML} is not a link of the studio`);
  window.location.hash = address;
}

const heading = (name: string | RegExp) => screen.findByRole("heading", { level: 1, name });

/** The rows of a table, each as the text of its cells. */
function rowsOf(table: HTMLElement): string[][] {
  return within(table)
    .getAllByRole("row")
    .slice(1)
    .map((line) =>
      within(line)
        .getAllByRole("cell")
        .map((cell) => cell.textContent ?? ""),
    );
}

/** The value beside a label in a list of fields. */
function field(label: string, within: ParentNode = document): string | null {
  const term = [...within.querySelectorAll("dt")].find((term) => term.textContent === label);
  return term?.nextElementSibling?.textContent ?? null;
}

const texts = (selector: string) => [...document.querySelectorAll(selector)].map((found) => found.textContent ?? "");

const servers: Server[] = [];
const projects: string[] = [];

afterEach(() => {
  cleanup();
  window.location.hash = "";
});

afterAll(async () => {
  for (const server of servers) await server.stop();
  for (const project of projects) rmSync(project, { recursive: true, force: true });
});

async function start(scenario: string, seed: number): Promise<Server> {
  const project = copyProject(scenario);
  projects.push(project);
  const server = await serve(project, seed);
  servers.push(server);
  return server;
}

describe("the door", () => {
  let server: Server;

  beforeAll(async () => {
    server = await start(companies, 11);
  }, SEEDING);

  test("asks for the token when the one of `spool dev` is refused, and keeps the one that works", async () => {
    const storage = open(server, "", memory());
    expect(await screen.findByText(/This server needs its bearer token/)).toBeTruthy();
    expect(screen.getByText(server.url)).toBeTruthy();
    expect(document.title).toBe("Spool studio");

    const token = screen.getByLabelText("Token") as HTMLInputElement;
    expect(token.type).toBe("password");
    fireEvent.change(token, { target: { value: "not-the-token" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText("The server refused that token.")).toBeTruthy();

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

  test("the overview counts what the store holds and what waits for someone", async () => {
    open(server, "");
    await heading("Overview");
    await screen.findByText("Entity type company");
    expect(texts(".card")).toEqual(["39observations", "16identities", "5without an identity", "1review item open"]);
    expect(field("Head")).toBe("log position 7");
    expect(field("Declaration")).toBe("version 1");
    expect(document.title).toBe("Overview · Spool studio");

    const drafts = await server.client.drafts({ status: "open" });
    const draft = document.querySelector("dd a[href^='#/drafts/']");
    expect(draft?.getAttribute("href")).toBe(`#/drafts/${drafts[0]!.id}`);

    // The same numbers stand beside their screens in the list of them.
    const nav = within(screen.getByRole("navigation"));
    expect(nav.getByRole("link", { name: /^Review/ }).textContent).toBe("Review1");
    expect(nav.getByRole("link", { name: /^Drafts/ }).textContent).toBe("Drafts1");
    expect(nav.getByRole("link", { name: "Overview" }).getAttribute("aria-current")).toBe("page");
  });

  test("a run that left a draft says what it made and where the draft is", async () => {
    open(server, "#/runs");
    await heading("Runs");
    const table = await screen.findByRole("table");
    const lines = rowsOf(table);
    expect(lines).toHaveLength(10);
    expect(lines[0]?.slice(0, 3)).toEqual(["10", "reconcile", "succeeded"]);
    expect(lines[1]?.slice(0, 4)).toEqual(["9", "match", "drafted", "company, rules"]);

    follow(within(table).getByRole("link", { name: "9" }));
    await heading("Run 9");
    await screen.findByText("What it made of them");
    expect(field("Status")).toBe("drafted");
    expect(field("Examined")).toBe("5 observations, 1 of them left for review before");
    expect(field("Placed")).toBe("4 observations, 1 identity created");
    expect(field("Left out")).toBe("1 observation in 1 review item");
    expect(field("Draft")).toMatch(/ was left for review$/);

    const drafts = await server.client.drafts({ status: "open" });
    const draft = document.querySelector("dd a[href^='#/drafts/']")!;
    expect(draft.getAttribute("href")).toBe(`#/drafts/${drafts[0]!.id}`);
    follow(draft);
    await heading("Draft");
    await screen.findByText("Operations (5)");
    expect(field("Status")).toBe("open");
    expect(field("From")).toBe("match run 9, reconcile run 10");
    expect(field("Would place")).toBe("4 observations, 1 identity created");
    // An operation names an observation by its source and key once it is read.
    await screen.findByRole("link", { name: "registry:X0000016" });
    const operations = texts(".ops li");
    expect(operations.filter((text) => /^place observation registry:X\d+ in 0/.test(text))).toHaveLength(4);
    expect(operations.filter((text) => text.startsWith("create company identity "))).toHaveLength(1);
  });

  test("the runs can be narrowed, and the address says how", async () => {
    open(server, "#/runs");
    await heading("Runs");
    await screen.findByRole("table");
    fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "ingest" } });
    expect(window.location.hash).toBe("#/runs?kind=ingest");
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(4));
    expect(rowsOf(screen.getByRole("table")).every((line) => line[1] === "ingest")).toBe(true);
    expect((screen.getByLabelText("Kind") as HTMLSelectElement).value).toBe("ingest");
  });

  test("the identities are listed with how many observations each holds", async () => {
    open(server, "#/identities");
    await heading("Identities");
    const lines = rowsOf(await screen.findByRole("table"));
    expect(lines).toHaveLength(16);
    expect(lines.reduce((sum, line) => sum + Number(line[2]), 0)).toBe(34);
  });

  test("an identity shows its members and the constraints on them", async () => {
    const split = await row(server.client, "registry", "X0000010");
    open(server, `#/identities/${split.identity}`);
    await heading("Identity");
    await screen.findByText("Members (1)");
    expect(texts("code.whole")).toEqual([split.identity!]);
    // The constraint names an observation that another identity holds.
    await screen.findByRole("link", { name: "crm:c-110" });
    const [members, constraints] = screen.getAllByRole("table").map(rowsOf);
    expect(members?.map((line) => [line[1], line[3]])).toEqual([["registry:X0000010", "log position 5"]]);
    // Splitting it out said the two are not one company, so that no run joins them again.
    expect(constraints).toEqual([["cannot link", "crm:c-110", "registry:X0000010", "log position 5"]]);
    expect(texts(".history > li")).toHaveLength(1);
    expect(texts(".history > li")[0]).toContain("“the Bristol company is another company”");
    expect(texts(".history > li")[0]).toMatch(/move observation registry:X0000010 from 0\S+ to 0/);
  });

  test("an identity shows every changeset that touched it", async () => {
    const atlas = await row(server.client, "crm", "c-109");
    open(server, `#/identities/${atlas.identity}`);
    await heading("Identity");
    await screen.findByText("History");
    await screen.findAllByRole("link", { name: "crm:c-109" });

    // It was made by the engine, merged away by the reviewer, and brought
    // back when the merge was undone.
    const history = texts(".history > li");
    expect(history).toHaveLength(3);
    expect(history[0]).toMatch(/^Log position 1 \S+ \S+ by engine, drafted by reconcile run 4/);
    expect(history[0]).toContain("place observation crm:c-109 in 0");
    expect(history[1]).toMatch(/^Log position 2 \S+ \S+ by user:studio-demo“same name, taken for one company”merge 0/);
    expect(history[2]).toMatch(/^Log position 4 \S+ \S+ by user:studio-demo“they are two companies after all”/);
    expect(history[2]).toContain("reinstate identity 0");
    expect(history[2]).toMatch(/move observation crm:c-109 from 0\S+ to 0/);
    expect(document.querySelector(".history a[href='#/log/4']")?.textContent).toBe("Log position 4");
  });

  test("an identity that was merged away leads to the one that stands for it", async () => {
    const { changesets } = await server.client.changesets({ after: 5, limit: 1 });
    const merge = changesets[0]!.changeset.ops.find((op) => op.op === "merge");
    if (merge?.op !== "merge") throw new Error("log position 6 has no merge");
    open(server, `#/identities/${merge.from}`);
    await heading("Identity");
    await screen.findByText("Members (3)");
    expect(field("Merged into")).toMatch(/^0\S+\. The members below are its members\.$/);
    expect(document.querySelector("dd a[href^='#/identities/']")?.getAttribute("href")).toBe(
      `#/identities/${merge.into}`,
    );
    // The reviewer said two of them are one company, and the identity says so.
    expect(rowsOf(screen.getAllByRole("table")[1]!)).toEqual([
      ["must link", "crm:c-111", "registry:X0000011", "log position 6"],
    ]);
  });

  test("an identity that is not there is said not to be", async () => {
    open(server, "#/identities/00000000-0000-7000-8000-000000000000");
    await heading("Identity");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "identity 00000000-0000-7000-8000-000000000000 does not exist",
    );
  });

  test("the observations no identity holds can be listed, and two of them explained", async () => {
    open(server, "#/observations");
    await heading("Observations");
    await screen.findByRole("table");
    fireEvent.click(screen.getByLabelText("Without an identity"));
    expect(window.location.hash).toBe("#/observations?unassigned=true");
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(5));
    const lines = rowsOf(screen.getByRole("table"));
    expect(lines.map((line) => line[1]).sort()).toEqual([
      "crm:c-117",
      "registry:X0000015",
      "registry:X0000016",
      "registry:X0000017",
      "registry:X0612001",
    ]);
    expect(lines.every((line) => line[4] === "none")).toBe(true);

    const ticks = screen.getAllByRole("checkbox", { name: /^Choose observation/ });
    fireEvent.click(ticks[0]!);
    expect(screen.queryByRole("link", { name: "Explain the two that are ticked" })).toBeNull();
    fireEvent.click(ticks[1]!);
    follow(screen.getByRole("link", { name: "Explain the two that are ticked" }));
    await heading("Explain");
    expect((await screen.findByText("Not both placed")).className).toContain("word");
  });

  test("the observations can be narrowed to a source and a key", async () => {
    open(server, "#/observations");
    await heading("Observations");
    await screen.findByRole("table");
    fireEvent.change(screen.getByLabelText("Source"), { target: { value: "crm" } });
    fireEvent.change(screen.getByLabelText("Key"), { target: { value: "c-104" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(window.location.hash).toBe("#/observations?source=crm&key=c-104");
    // The row came in both batches, with a change, so it has two versions.
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(2));
  });

  test("an observation shows what was ingested and the review item it waits in", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    open(server, `#/observations/${waiting.id}`);
    await heading("Observation");
    await screen.findByText("Payload");
    expect(field("Source")).toBe("crm");
    expect(field("Key")).toBe("c-117");
    expect(field("Identity")).toBe("none");
    expect(field("Review")).toBe("item 1 possible merge open");
    for (const [name, value] of Object.entries(waiting.payload as Record<string, string>)) {
      expect(field(name)).toBe(value);
    }

    follow(screen.getByRole("link", { name: "item 1" }));
    await heading("Review item 1");
    await screen.findByText("Identities they matched");
    expect(field("Kind")).toBe("possible merge");
    expect(field("Status")).toBe("open");
    expect(field("Raised by")).toBe("reconcile run 7");
    expect(rowsOf(screen.getByRole("table")).map((line) => [line[1], line[3]])).toEqual([["crm:c-117", "none"]]);
    expect(document.querySelectorAll("ul.plain a[href^='#/identities/']")).toHaveLength(2);
  });

  test("an observation is explained with another that is named by its source and key", async () => {
    const waiting = await row(server.client, "registry", "X0612001");
    const other = await row(server.client, "registry", "X0000009");
    open(server, `#/observations/${waiting.id}`);
    await heading("Observation");
    const ask = await screen.findByLabelText("Why is it with another observation, or apart from it?");
    fireEvent.change(ask, { target: { value: "registry:nothing" } });
    fireEvent.click(within(screen.getByRole("main")).getByRole("button", { name: "Explain" }));
    expect((await screen.findByRole("alert")).textContent).toBe("No observation is named registry:nothing.");

    fireEvent.change(ask, { target: { value: "registry:X0000009" } });
    fireEvent.click(within(screen.getByRole("main")).getByRole("button", { name: "Explain" }));
    await waitFor(() => expect(window.location.hash).toBe(`#/explain?a=${waiting.id}&b=${other.id}`));
    await screen.findByText("Not both placed");
    await screen.findByText("No block rule holds, so a run of the rules would not compare the two.");
  });

  test("two records a reviewer merged and then parted are explained by what happened", async () => {
    open(server, "#/explain");
    await heading("Explain");
    fireEvent.click(within(screen.getByRole("main")).getByRole("button", { name: "Explain" }));
    expect((await screen.findByRole("alert")).textContent).toBe("An explanation is about two observations.");

    fireEvent.change(screen.getByLabelText("One observation"), { target: { value: "crm:c-108" } });
    fireEvent.change(screen.getByLabelText("The other"), { target: { value: "crm:c-109" } });
    fireEvent.click(within(screen.getByRole("main")).getByRole("button", { name: "Explain" }));

    await screen.findByText("Different identities");
    const a = await row(server.client, "crm", "c-108");
    const b = await row(server.client, "crm", "c-109");
    expect(window.location.hash).toBe(`#/explain?a=${a.id}&b=${b.id}`);

    // The sentences are the engine's own, with what it put in backticks set as code.
    const explanation = await server.client.explain(a.id, b.id);
    expect(texts(".outcome p")).toEqual(explanation.summary.map((sentence) => sentence.replaceAll("`", "")));
    expect(texts(".outcome p").join(" ")).toContain("they are two companies after all");
    expect(texts(".outcome p code")).toContain("crm:c-109");
    expect(texts(".outcome p").join(" ")).not.toContain("`");

    const [features, rules, comparisons] = screen.getAllByRole("table").map(rowsOf);
    expect(features).toContainEqual(["name_key", "atlas trading", "atlas trading"]);
    expect(features).toContainEqual(["trading_key", "-", "-"]);
    // Only a value both have is marked as shared: two that are missing are not.
    expect(texts("tr.same td:first-child")).toEqual(["name_key"]);
    expect(rules).toContainEqual(["block", "same_name", "l.name_key = r.name_key", "holds"]);
    expect(rules).toContainEqual(["match", "same_name_and_country", expect.any(String), "does not hold"]);
    expect(rules).toContainEqual([
      "block",
      "same_domain",
      "l.domain = r.domain",
      "cannot be decided: a feature it reads is null",
    ]);
    expect(screen.getByText("A run of the rules would compare the two and make no judgement.")).toBeTruthy();
    expect(comparisons?.map((line) => [line[0], line[2], line[4], line[5]])).toEqual([
      ["3", "rules", "same_name", "no judgement"],
    ]);
  });

  test("two versions of one row are explained by what joins them", async () => {
    const { observations } = await server.client.observations({ source: "crm", key: "c-104" });
    expect(observations).toHaveLength(2);
    open(server, `#/explain?a=${observations[0]!.id}&b=${observations[1]!.id}`);
    await heading("Explain");
    await screen.findByText("The same identity");
    const sides = [...document.querySelectorAll(".side")];
    expect(sides.map((side) => field("Observation", side))).toEqual(["crm:c-104", "crm:c-104"]);
    expect(field("Identity", sides[0])).toBe(field("Identity", sides[1]));
    expect((screen.getByLabelText("One observation") as HTMLInputElement).value).toBe(observations[0]!.id);
  });

  test("an observation that is not there is said not to be", async () => {
    const a = await row(server.client, "crm", "c-108");
    open(server, `#/explain?a=${a.id}&b=00000000-0000-4000-8000-000000000000`);
    await heading("Explain");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "there is no observation 00000000-0000-4000-8000-000000000000",
    );
  });

  test("the log lists every changeset, and one shows what it did", async () => {
    const { changesets } = await server.client.changesets({ after: 3, limit: 1 });
    open(server, "#/log");
    await heading("Log");
    const table = await screen.findByRole("table");
    const lines = rowsOf(table);
    expect(lines.map((line) => line[0])).toEqual(["1", "2", "3", "4", "5", "6", "7"]);
    expect(lines[0]).toEqual(["1", "engine", "40", "reconcile run 4", "reconcile new observations"]);
    expect(lines[3]).toEqual(["4", REVIEWER, "3", "-", "they are two companies after all"]);

    follow(within(table).getByRole("link", { name: "4" }));
    await heading("Log position 4");
    await screen.findByText("Operations (3)");
    expect(field("Actor")).toBe(REVIEWER);
    expect(field("Reason")).toBe("“they are two companies after all”");
    expect(field("Written against")).toBe("log position 3");
    expect(field("Changeset")).toBe(changesets[0]!.changeset.id);
    expect(field("Content hash")).toBe(changesets[0]!.content_hash);
    await screen.findByRole("link", { name: "crm:c-109" });
    const operations = texts(".ops li");
    expect(operations[0]).toMatch(/^reinstate identity 0/);
    expect(operations[1]).toMatch(/^move observation crm:c-109 from 0\S+ to 0/);
  });

  test("a log position past the end is said not to be there", async () => {
    open(server, "#/log/99");
    await heading("Log position 99");
    expect((await screen.findByRole("alert")).textContent).toBe("The log has no position 99.");
  });

  test("an ID, or a source and key, typed at the top goes to what it names", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const drafts = await server.client.drafts({ status: "open" });
    open(server, "#/runs");
    await heading("Runs");
    const find = () => screen.getByRole("searchbox", { name: "Find by ID, or by source and key" });
    fireEvent.change(find(), { target: { value: " crm:c-111 " } });
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await waitFor(() => expect(window.location.hash).toBe(`#/observations/${orchard.id}`));
    await heading("Observation");

    fireEvent.change(find(), { target: { value: orchard.identity! } });
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await waitFor(() => expect(window.location.hash).toBe(`#/identities/${orchard.identity}`));
    await heading("Identity");
    await screen.findByText("Constraints on its members");

    fireEvent.change(find(), { target: { value: drafts[0]!.id } });
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await waitFor(() => expect(window.location.hash).toBe(`#/drafts/${drafts[0]!.id}`));
    await heading("Draft");

    fireEvent.change(find(), { target: { value: "crm:nothing" } });
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Nothing here is named crm:nothing.");
    expect(window.location.hash).toBe(`#/drafts/${drafts[0]!.id}`);
  });

  test("an address no screen has says so", async () => {
    for (const address of ["#/nowhere", "#/runs/9/more", "#/explain/this"]) {
      open(server, address);
      await heading("No such screen");
      expect(screen.getByRole("link", { name: "Go to the overview." }).getAttribute("href")).toBe("#/");
      cleanup();
    }
  });

  test("it only reads", () => {
    expect(calls.length).toBeGreaterThan(60);
    expect(calls.filter((call) => !call.startsWith("GET /v1/"))).toEqual([]);
  });

  test("what changed since a screen was read is shown when it is read again", async () => {
    open(server, "#/review");
    await heading("Review");
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(1));
    expect(rowsOf(screen.getByRole("table"))[0]).toEqual([
      "1",
      "possible merge",
      "open",
      "company",
      "1 observation",
      "2 identities",
      "run 7",
    ]);

    // Someone else settles the item while the screen is open.
    await server.client.dismissReview(1, { actor: REVIEWER, reason: "looked at it" });
    expect(rowsOf(screen.getByRole("table"))[0]?.[2]).toBe("open");
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))[0]?.[2]).toBe("dismissed"));
    await waitFor(() =>
      expect(within(screen.getByRole("navigation")).getByRole("link", { name: /^Review/ }).textContent).toBe("Review"),
    );

    follow(within(screen.getByRole("table")).getByRole("link", { name: "1" }));
    await heading("Review item 1");
    await screen.findByText("Observations left without an identity");
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
    open(server, "#/documents");
    await heading("Documents");
    const table = await screen.findByRole("table");
    expect(rowsOf(table).length).toBeGreaterThan(5);
    follow(within(table).getByRole("link", { name: "news:09-falcon-watch.txt" }));

    await heading("Document");
    await screen.findByText("Mentions (3)");
    const { documents } = await server.client.documents({ source: "news", key: "09-falcon-watch.txt" });
    const record = await server.client.document(documents[0]!.id);
    expect(texts("pre.document")).toEqual([record.text!]);
    expect(texts("pre.document a.mention")).toEqual(["The Falcon Ltd", "Orchard Ltd", "NORTHWIND TRADERS LTD"]);
    expect(field("Read by")).toMatch(/^suffix_ts in run \d+ \(3 mentions\)$/);
    expect(rowsOf(screen.getByRole("table")).map((line) => line.slice(1, 4))).toEqual([
      ["The Falcon Ltd", "0 to 14", "company"],
      ["Orchard Ltd", "70 to 81", "company"],
      ["NORTHWIND TRADERS LTD", "122 to 143", "company"],
    ]);

    // A mention is an observation: its screen shows where it was found.
    follow(document.querySelectorAll("pre.document a.mention")[1]!);
    await heading("Observation");
    await screen.findByText("Where it was found");
    expect(texts(".excerpt mark")).toEqual(["Orchard Ltd"]);
    expect(texts(".excerpt")[0]).toContain("Its maker, Orchard Ltd, said more stock would arrive in March.");
    expect(field("extractor")).toBe("suffix_ts");
    expect(field("name")).toBe("Orchard Ltd");

    // And leads back to the document, at the mention.
    const back = screen.getByRole("link", { name: "news:09-falcon-watch.txt" });
    expect(back.getAttribute("href")).toBe(`#/documents/${record.id}?at=70`);
    follow(back);
    await heading("Document");
    await screen.findByText("Mentions (3)");
    expect(texts("a.mention.here")).toEqual(["Orchard Ltd"]);
  });

  test("a document's text is taken apart once, however many mentions it has", async () => {
    const { documents } = await server.client.documents({ source: "news", key: "09-falcon-watch.txt" });
    const record = await server.client.document(documents[0]!.id);
    // Counting a text's characters reads all of it, so a long document with
    // many mentions must not be read once for each of them.
    const taken = spyOn(Array, "from");
    const apart = () => taken.mock.calls.filter(([value]) => value === record.text).length;
    try {
      open(server, `#/documents/${record.id}`);
      await heading("Document");
      await screen.findByText("Mentions (3)");
      expect(apart()).toBe(1);

      // Ticking a mention shows the screen again, with the same text.
      const ticks = screen.getAllByRole("checkbox", { name: /^Choose observation/ });
      fireEvent.click(ticks[0]!);
      fireEvent.click(ticks[1]!);
      await screen.findByRole("link", { name: "Explain the two that are ticked" });
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
    const first = await server.client.observation(view.members[0]!.observation);
    const found = first.evidence as { key: string; start: number; end: number };
    const name = `news:${found.key}[${found.start}-${found.end}]`;

    open(server, `#/identities/${largest.id}`);
    await heading("Identity");
    await screen.findByText(`Members (${largest.members})`);
    // Each is named by its document and where in it the mention is, in the
    // members and in the history alike.
    await waitFor(() => expect(screen.getAllByRole("link", { name })).toHaveLength(2));
    const members = rowsOf(screen.getAllByRole("table")[0]!).map((member) => member[1]!);
    expect(members).toHaveLength(largest.members);
    expect(members.every((member) => /^news:\S+\.txt\[\d+-\d+\]$/.test(member))).toBe(true);
  });

  test("the mentions of one document can be listed as observations", async () => {
    const { documents } = await server.client.documents({ source: "news", key: "09-falcon-watch.txt" });
    open(server, `#/observations?document=${documents[0]!.id}`);
    await heading("Observations");
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(3));
    expect(
      rowsOf(screen.getByRole("table"))
        .map((line) => line[1])
        .sort(),
    ).toEqual([
      "news:09-falcon-watch.txt[0-14]",
      "news:09-falcon-watch.txt[122-143]",
      "news:09-falcon-watch.txt[70-81]",
    ]);
  });

  test("the overview says which extractors the project has, and nothing waits", async () => {
    open(server, "");
    await heading("Overview");
    await screen.findByText("Extractors");
    expect(rowsOf(screen.getByRole("table")).map((line) => line[0])).toContain("suffix_ts");
    expect(field("Documents")).toBe(String((await server.client.info()).store.documents));
    const nav = within(screen.getByRole("navigation"));
    expect(nav.getByRole("link", { name: "Review" }).textContent).toBe("Review");
    expect(nav.getByRole("link", { name: "Drafts" }).textContent).toBe("Drafts");
  });
});
