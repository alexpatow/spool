// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The documents an extractor read, and the mentions it found in them.

import { afterAll, afterEach, beforeAll, describe, expect, spyOn, test } from "bun:test";

import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { Server } from "spool-testing";

import {
  SEEDING,
  STOPPING,
  counted,
  field,
  follow,
  heading,
  here,
  open,
  parts,
  rowsOf,
  screens,
  start,
  stopAll,
  texts,
} from "./harness.tsx";
import { shownFor } from "./looking.tsx";
import { news, seedNews } from "./seed.ts";

afterEach(() => {
  cleanup();
  window.history.replaceState(null, "", "/");
});

afterAll(stopAll, STOPPING);

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
