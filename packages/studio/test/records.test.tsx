// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The records, and the screen of one.

import { describe, expect, test } from "bun:test";

import { fireEvent, screen, waitFor } from "@testing-library/react";

import { field, follow, heading, here, open, parts, rowsOf, texts } from "./harness.tsx";
import { aStoreOfCompanies, lookingChangedNothing, server, shownFor } from "./looking.tsx";
import { row } from "./seed.ts";

describe("company records a reviewer corrected", () => {
  aStoreOfCompanies();

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

  test("a record shows what was loaded and the review item it waits in", async () => {
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

  lookingChangedNothing(8);
});
