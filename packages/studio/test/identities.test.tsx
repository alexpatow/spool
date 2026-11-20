// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The identities, and the screen of one.

import { describe, expect, test } from "bun:test";

import { fireEvent, screen, waitFor, within } from "@testing-library/react";

import { counted, heading, here, open, parts, rowsOf, texts } from "./harness.tsx";
import { aStoreOfCompanies, lookingChangedNothing, server } from "./looking.tsx";
import { row } from "./seed.ts";

describe("company records a reviewer corrected", () => {
  aStoreOfCompanies();

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

  test("identity search defaults to relevance and shows why each result matched", async () => {
    open(server, "/identities");
    await heading("Identities");
    await screen.findByRole("table");
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: " halden energy " } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    expect(here()).toBe("/identities?q=halden+energy");
    await waitFor(() => expect(rowsOf(screen.getByRole("table")).map((line) => line[0])).toEqual(["Halden Energy"]));
    expect(screen.getByRole("columnheader", { name: "Name" }).getAttribute("aria-sort")).toBe("none");
    expect(screen.getByRole("cell", { name: "Name match" })).toBeTruthy();

    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), {
      target: { value: "acorn tool company ltd" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    expect(await screen.findByRole("cell", { name: "Alias “ACORN TOOL COMPANY LTD”" })).toBeTruthy();

    // A word of a value that is not the name finds it too, whatever its case.
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "X0000004" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() =>
      expect(rowsOf(screen.getByRole("table")).map((line) => line[0])).toEqual(["Kestrel Analytics"]),
    );
    expect(screen.getByRole("cell", { name: "Record field “number”" })).toBeTruthy();

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
    expect(records[0]?.[5]).toBe("log position 5 by agent:api “the Bristol company is another company”");
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
    expect(history[1]).toMatch(/^Log position 2 \S+ \S+ by agent:api “same name, taken for one company”merge /);
    expect(history[1]).toEndWith(`merge ${here} into ${there}`);
    expect(history[2]).toMatch(/^Log position 4 \S+ \S+ by agent:api “they are two companies after all”/);
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

  lookingChangedNothing(16);
});
