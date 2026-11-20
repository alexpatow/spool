// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The review items.

import { describe, expect, test } from "bun:test";

import { fireEvent, screen, waitFor, within } from "@testing-library/react";

import { field, follow, heading, here, open, parts, screens } from "./harness.tsx";
import { aStoreOfCompanies, lookingChangedNothing, server } from "./looking.tsx";
import { row } from "./seed.ts";

describe("company records a reviewer corrected", () => {
  aStoreOfCompanies();

  test("a review item shows what it is decided from: the record, the identities it matched, and why", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const found = await server.client.reviewCase(1);
    open(server, "/review");
    await heading("Review");
    const card = within(await screen.findByRole("article", { name: "Review item 1" }));
    expect(card.getByRole("heading", { level: 2 }).textContent).toBe(
      "Pioneer Supplies matches both Pioneer Supplies and PIONEER SUPPLIES LTD. Are they the same thing?",
    );
    expect(parts("lead", card.getByRole("heading", { level: 2 }).closest("article")!)[0]).toContain(
      "Until someone answers, it has no identity, and every match run of company looks at it again.",
    );

    // The two identities side by side: what each is called, how many records it has, some of them, and what links it.
    const sides = card.getAllByRole("region", { name: "The identities" })[0]!;
    expect(parts("size", sides)).toEqual(["2 recordscrm1registry1", "1 recordregistry1"]);
    expect(parts("members", sides)).toEqual([
      expect.stringContaining("crm:c-110"),
      expect.stringContaining("registry:X0000010"),
    ]);
    // What tells the two apart, from what their records say.
    const differs = parts("differs", sides);
    expect(differs).toHaveLength(2);
    expect(differs[0]).toContain("city Leeds");
    expect(differs[1]).toContain("city Bristol");
    expect(parts("links", sides)).toEqual([
      "Linked by 2 matches with 2 of its records, by same_name_and_country.",
      "Linked by 1 match with 1 of its records, by same_name_and_country.",
    ]);

    // Before anything is done, each answer says what it does.
    const answers = card.getByRole("region", { name: "Answers" });
    expect([...answers.querySelectorAll("[data-answer]")].map((answer) => answer.getAttribute("data-answer"))).toEqual(
      ["merge", "assign", "new", "dismiss", "leave"],
    );
    const does = (name: string) => answers.querySelector(`[data-answer=${name}]`)!.textContent!;
    expect(does("assign")).toContain(
      "Pioneer Supplies is placed in the one you choose, which then has 3 records for Pioneer Supplies and 2 records for PIONEER SUPPLIES LTD.",
    );
    expect(does("assign")).toContain("a later record that matches more than one of them is asked about again");
    expect(does("assign")).toContain("No match run asks about them then, only a rematch (spool match --rematch).");
    expect(does("new")).toContain("Pioneer Supplies becomes an identity of its own");
    expect(does("dismiss")).toContain("no later match run looks at it again, a rematch included");
    expect(does("leave")).toContain("Every match run of company looks at it again");
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

    // Merging the two is not offered: they are kept apart, and the answer says so.
    expect(card.getAllByRole("button").map((button) => button.textContent)).toEqual([
      "Assign to Pioneer Supplies",
      "Assign to PIONEER SUPPLIES LTD",
      "New identity",
      "Dismiss",
    ]);
    expect(does("merge")).toContain("Not possible: a cannot_link constraint is between records of these identities");
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

  lookingChangedNothing(4);

  test("what changed since a screen was read is shown when it is read again", async () => {
    open(server, "/review");
    await heading("Review");
    await screen.findByRole("article", { name: "Review item 1" });
    expect(screens().getByRole("link", { name: /^Review/ }).textContent).toBe("Review1");

    // Someone else settles the item while the screen is open.
    await server.client.dismissReview(1, { reason: "looked at it" });
    expect(screen.getByRole("article", { name: "Review item 1" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    // It no longer waits, so it leaves the list of those that do.
    await waitFor(() => expect(parts("empty")).toHaveLength(1));
    expect(parts("empty")[0]).toStartWith("No open review items.");
    await waitFor(() => expect(screens().getByRole("link", { name: /^Review/ }).textContent).toBe("Review"));

    follow(within(screen.getByRole("list", { name: "Status" })).getByRole("link", { name: "Dismissed" }));
    const card = within(await screen.findByRole("article", { name: "Review item 1" }));
    expect(card.getByText(/^Dismissed by/).textContent).toMatch(/^Dismissed by agent:api at \S+ \S+ “looked at it”\.$/);
    // Nothing is left to decide.
    expect(card.queryAllByRole("button")).toEqual([]);

    follow(card.getByRole("link", { name: "Item 1" }));
    await heading("Review item 1");
    await screen.findByRole("article", { name: "Review item 1" });
    expect(field("Status")).toBe("dismissed");
    expect(field("Dismissed by")).toMatch(/^agent:api at \S+ \S+ “looked at it”$/);
  });
});
