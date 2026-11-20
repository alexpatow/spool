// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The runs, and the lists that are narrowed and sorted in their address.

import { describe, expect, test } from "bun:test";

import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";

import { calls, counted, field, follow, heading, here, idle, open, rowsOf, texts } from "./harness.tsx";
import { CALLED, aStoreOfCompanies, lookingChangedNothing, server } from "./looking.tsx";

describe("company records a reviewer corrected", () => {
  aStoreOfCompanies();

  test("a run that left a draft says what it made and where the draft is", async () => {
    open(server, "/runs");
    await heading("Runs");
    const table = await screen.findByRole("table");
    const lines = rowsOf(table);
    expect(lines).toHaveLength(10);
    expect(lines[0]?.slice(0, 5)).toEqual(["Run 10", "reconcile", "succeeded", "", "agent:api"]);
    expect(lines[1]?.slice(0, 5)).toEqual(["Run 9", "match", "drafted", "company, rules", "agent:api"]);

    follow(within(table).getByRole("link", { name: "Run 9" }));
    await heading("Run 9");
    await screen.findByText("Resolution");
    expect(field("Status")).toBe("drafted");
    expect(field("Started by")).toBe("agent:api");
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
    fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "load" } });
    expect(here()).toBe("/runs?kind=load");
    await waitFor(() => expect(rowsOf(screen.getByRole("table"))).toHaveLength(4));
    expect(rowsOf(screen.getByRole("table")).every((line) => line[1] === "load")).toBe(true);
    expect((screen.getByLabelText("Kind") as HTMLSelectElement).value).toBe("load");
  });

  test("a list read in pages is sorted by the server by the column whose header is clicked", async () => {
    const header = (name: string) => screen.getByRole("columnheader", { name: new RegExp(`^${name}`) });
    const sortedBy = () =>
      [...document.querySelectorAll("th[aria-sort]")]
        .filter((head) => head.getAttribute("aria-sort") !== "none")
        .map((head) => `${head.textContent} ${head.getAttribute("aria-sort")}`);

    open(server, "/identities");
    await heading("Identities");
    await screen.findByRole("table");
    // By name until another order is chosen.
    expect(sortedBy()).toEqual(["Name ascending"]);
    const names = rowsOf(screen.getByRole("table")).map((line) => line[0]!);
    expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" })));

    // A number is sorted the largest first, and the order is in the address.
    calls.length = 0;
    fireEvent.click(within(header("Records")).getByRole("button"));
    expect(here()).toBe("/identities?sort=records&order=desc");
    await waitFor(() => expect(sortedBy()).toEqual(["Records descending"]));
    await idle();
    expect(calls).toContain("GET /v1/identities");
    const { identities } = await server.client.identities({ sort: "records", order: "desc", limit: 100 });
    const records = rowsOf(screen.getByRole("table")).map((line) => Number(line[2]));
    expect(records).toEqual(identities.map((identity) => identity.members));
    expect(records[0]).toBeGreaterThan(records.at(-1)!);

    // Another click turns it around.
    fireEvent.click(within(header("Records")).getByRole("button"));
    expect(here()).toBe("/identities?sort=records&order=asc");
    await waitFor(() => expect(rowsOf(screen.getByRole("table")).map((line) => Number(line[2]))).toEqual(
      [...records].reverse().sort((a, b) => a - b),
    ));

    // The runs are the newest first, and a list cut at a limit is sorted before it is cut.
    cleanup();
    open(server, "/runs?limit=3");
    await heading("Runs");
    await screen.findByRole("table");
    expect(sortedBy()).toEqual(["Run descending"]);
    fireEvent.click(within(header("Kind")).getByRole("button"));
    expect(here()).toBe("/runs?limit=3&sort=kind&order=asc");
    const kinds = (await server.client.runs({ sort: "kind", limit: 3 })).map((run) => run.kind);
    expect(kinds[0]).toBe("load");
    await waitFor(() => expect(rowsOf(screen.getByRole("table")).map((line) => line[1])).toEqual(kinds));
  });

  lookingChangedNothing(2);
});
