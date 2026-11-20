// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The box that finds things, over any screen.

import { describe, expect, test } from "bun:test";

import { fireEvent, screen, waitFor } from "@testing-library/react";

import { heading, here, idle, open } from "./harness.tsx";
import { aStoreOfCompanies, lookingChangedNothing, server } from "./looking.tsx";
import { row } from "./seed.ts";

describe("company records a reviewer corrected", () => {
  aStoreOfCompanies();

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
    // Tokens is among them, since an admin is looking.
    expect(listed()).toContain("Tokens");
    expect(listed()).toHaveLength(11);

    fireEvent.change(box, { target: { value: "Pioneer" } });
    await screen.findByRole("option", { name: /^PIONEER SUPPLIES LTD Name match · company · 1 record/ });
    await idle();
    expect(listed()).toEqual([
      "Pioneer SuppliesName match · company · 2 recordscrm1registry1",
      "PIONEER SUPPLIES LTDName match · company · 1 recordregistry1",
      "Pioneer Suppliesc-110crm",
      "Pioneer Suppliesc-117crm",
      "PIONEER SUPPLIES LTDX0000009registry",
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

  lookingChangedNothing(12);
});
