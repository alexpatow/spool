// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The overview, and what is around every screen.

import { describe, expect, test } from "bun:test";

import { cleanup, fireEvent, screen, within } from "@testing-library/react";

import { THEME_KEY } from "../src/app.tsx";
import { calls, field, heading, here, memory, open, parts, rowsOf, screens } from "./harness.tsx";
import { aStoreOfCompanies, lookingChangedNothing, server } from "./looking.tsx";

describe("company records a reviewer corrected", () => {
  aStoreOfCompanies();

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

  test("a list read whole is sorted in the page by the column whose header is clicked", async () => {
    open(server, "");
    await heading("Overview");
    const table = await screen.findByRole("table", { name: "company sources" });
    const records = () => rowsOf(table).map((line) => Number(line[1]));
    const as = records();
    expect(within(table).getAllByRole("columnheader").every((head) => head.getAttribute("aria-sort") === "none")).toBe(
      true,
    );
    const button = within(within(table).getByRole("columnheader", { name: /^Records/ })).getByRole("button");
    fireEvent.click(button);
    expect(records()).toEqual([...as].sort((a, b) => b - a));
    expect(within(table).getByRole("columnheader", { name: /^Records/ }).getAttribute("aria-sort")).toBe("descending");
    fireEvent.click(button);
    expect(records()).toEqual([...as].sort((a, b) => a - b));
    // The page stays where it is: the order of a list read whole is not in the address.
    expect(here()).toBe("/");
  });

  test("the page can be made dark, and stays so", async () => {
    const names = memory();
    open(server, "", names);
    await heading("Overview");
    expect(document.documentElement.dataset.theme).toBe("light");
    fireEvent.click(screen.getByRole("button", { name: "Dark theme" }));
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
    expect(names.getItem(THEME_KEY)).toBe("dark");
    // Payload nothing with is a class on the page: its policy allows no style of its own.
    expect(document.querySelectorAll("style")).toHaveLength(0);
    cleanup();

    open(server, "", names);
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

  lookingChangedNothing(10);
});
