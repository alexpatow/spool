// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// Why two records are one thing or are not.

import { describe, expect, test } from "bun:test";

import { fireEvent, screen, waitFor, within } from "@testing-library/react";

import { field, heading, here, open, rowsOf, texts } from "./harness.tsx";
import { aStoreOfCompanies, lookingChangedNothing, server, shownFor } from "./looking.tsx";
import { row } from "./seed.ts";

describe("company records a reviewer corrected", () => {
  aStoreOfCompanies();

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

  lookingChangedNothing(8);
});
