// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The log, and the screen of one changeset.

import { describe, expect, test } from "bun:test";

import { screen, waitFor, within } from "@testing-library/react";

import { counted, field, follow, heading, open, rowsOf, texts } from "./harness.tsx";
import { aStoreOfCompanies, lookingChangedNothing, server } from "./looking.tsx";

describe("company records a reviewer corrected", () => {
  aStoreOfCompanies();

  test("the log lists every changeset, the newest first, and one shows what it did", async () => {
    const { changesets } = await server.client.changesets({ after: 3, limit: 1 });
    open(server, "/log");
    await heading("Log");
    const table = await screen.findByRole("table");
    const lines = rowsOf(table);
    expect(lines.map((line) => line[0])).toEqual(["7", "6", "5", "4", "3", "2", "1"]);
    expect(lines[6]).toEqual(["1", "engine", "40", "reconcile run 4", "agent:api", "reconcile new records"]);
    expect(lines[3]).toEqual(["4", "agent:api", "3", "-", "-", "they are two companies after all"]);

    follow(within(table).getByRole("link", { name: "4" }));
    await heading("Log position 4");
    await counted("Operations", 3);
    expect(field("Actor")).toBe("agent:api");
    expect(field("Started by")).toBeNull();
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

  lookingChangedNothing(3);
});
