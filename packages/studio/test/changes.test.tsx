// What a reviewer changes from the studio, against the real server: each
// change is made on a screen, and the server is asked what came of it.
// These are the changes to a draft and to a review item; those to records,
// identities and the log are in `changes-identities.test.tsx`.

import { describe, expect, test } from "bun:test";

import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";

import {
  aStoreForEachTest,
  asked,
  asking,
  cancel,
  changes,
  entry,
  head,
  make,
  refused,
  said,
  server,
} from "./changing.tsx";
import { author, calls, counted, field, follow, heading, idle, open, parts, screens, texts } from "./harness.tsx";
import { row } from "./seed.ts";

aStoreForEachTest();

describe("a draft", () => {
  test("is approved from its screen, and is in the log", async () => {
    const [draft] = await server.client.drafts({ status: "open" });
    open(server, `/drafts/${draft!.id}`);
    await heading("Draft");
    await counted("Operations", 5);
    await idle();

    // The dialog says what approving would do before it is done.
    const dialog = await asked("Approve");
    expect(dialog.getByText("Commits 5 operations: 4 records assigned, 1 identity created.")).toBeTruthy();
    expect(changes()).toEqual([]);

    // Pressed twice before the screen has shown the first press, it is made once.
    const form = dialog.getByRole("button", { name: "Approve" }).closest("form")!;
    act(() => {
      fireEvent.submit(form);
      fireEvent.submit(form);
    });
    expect(await said()).toBe("Committed at log position 8.");
    expect(screen.queryByRole("alert")).toBeNull();
    await waitFor(() => expect(field("Status")).toBe("committed"));
    expect(field("Committed at")).toBe("log position 8");
    // It is decided, so there is nothing left to press.
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Reject" })).toBeNull();
    await waitFor(() => expect(screens().getByRole("link", { name: /^Drafts/ }).textContent).toBe("Drafts"));

    expect((await server.client.draft(draft!.id)).draft.status).toBe("committed");
    expect(await head()).toBe(8);
    expect(changes()).toEqual([`POST /v1/drafts/${draft!.id}/approve`]);
  });

  test("that no longer holds is not committed, and the screen names the operations", async () => {
    const [draft] = await server.client.drafts({ status: "open" });
    const { changeset } = await server.client.draft(draft!.id);
    const placed = changeset.ops.find((op) => op.op === "assign");
    if (placed?.op !== "assign") throw new Error("the draft places nothing");
    const name = (await server.client.record(placed.record)).key;
    open(server, `/drafts/${draft!.id}`);
    await heading("Draft");
    await counted("Operations", 5);
    await idle();

    // Someone places one of its records elsewhere while it is read.
    const [elsewhere] = (await server.client.mintIds({ identities: 1 })).identities;
    await server.client.commit({
      ops: [
        { op: "create_identity", entity_type: "company", identity: elsewhere! },
        { op: "assign", record: placed.record, from: null, to: elsewhere! },
      ],
    });

    await make("Approve");
    const answer = await refused();
    expect(answer).toContain(
      `changeset rejected: operation 0 (assign): stale precondition: record ${placed.record} was expected to belong to no identity`,
    );
    expect(answer).toContain("Nothing was changed.");
    // The operation the engine names is shown as the list of them shows it.
    expect(texts("[role=alert] li")).toHaveLength(1);
    expect(texts("[role=alert] li")[0]).toMatch(/^Operation 0: assign record \S+ to 0/);
    await waitFor(() =>
      expect(texts("[data-part=ops] li")[0]).toMatch(
        new RegExp(`^assign record registry:${name} to .+ \\(…[0-9a-f]{4}\\)$`),
      ),
    );

    // The dialog stays, with why, and the screen behind it says nothing was done.
    expect(asking()).toBe(true);
    expect(screen.queryByRole("status")).toBeNull();
    expect(field("Status")).toBe("open");
    expect((await server.client.draft(draft!.id)).draft.status).toBe("open");
    expect(await head()).toBe(8);
  });

  test("is rejected with the reason the reviewer gave", async () => {
    const [draft] = await server.client.drafts({ status: "open" });
    open(server, `/drafts/${draft!.id}`);
    await heading("Draft");
    await counted("Operations", 5);
    await idle();

    await make("Reject", { Reason: " the Leeds record is not ours " });
    expect(await said()).toBe("Rejected.");
    await waitFor(() => expect(field("Status")).toBe("rejected"));
    expect(field("Rejected")).toMatch(/“the Leeds record is not ours”$/);
    expect(screen.queryByRole("button", { name: "Reject" })).toBeNull();

    const { draft: after } = await server.client.draft(draft!.id);
    expect(after.status).toBe("rejected");
    expect(after.rejected_reason).toBe("the Leeds record is not ours");
    expect(await head()).toBe(7);
  });
});

describe("a review item", () => {
  /** The item's screen, once it shows what the item is decided from. */
  async function item() {
    await heading("Review item 1");
    const card = await screen.findByRole("article", { name: "Review item 1" });
    await idle();
    return within(card);
  }

  test("is resolved by placing its record in an identity it matched", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const found = await server.client.reviewCase(1);
    const [, other] = found.candidates.map((candidate) => candidate.identity);
    open(server, "/review/1");
    const card = await item();

    // The button under an identity proposes that identity, and the dialog
    // lets the record go elsewhere: nowhere, another it matched, or one of its own.
    const dialog = await asked(`Assign to ${other!.label}`);
    expect(dialog.getByRole("heading").textContent).toBe("Assign Pioneer Supplies");
    const where = dialog.getByLabelText("crm:c-117") as HTMLSelectElement;
    expect([...where.options].map((option) => option.value)).toEqual([
      "",
      ...found.candidates.map((candidate) => candidate.identity.id),
      "new",
    ]);
    expect(where.value).toBe(other!.id);
    expect(changes()).toEqual([]);

    await make(`Assign to ${other!.label}`, { Reason: "the Bristol company" });
    expect(await said()).toBe("1 record assigned at log position 8.");
    await waitFor(() => expect(field("Status")).toBe("resolved"));
    expect(field("Resolved at")).toBe("log position 8");
    // It is decided, so there is nothing left to press.
    expect(card.queryAllByRole("button")).toEqual([]);
    await waitFor(() => expect(screens().getByRole("link", { name: /^Review/ }).textContent).toBe("Review"));

    expect((await server.client.record(waiting.id)).identity).toBe(other!.id);
    const { changeset } = await entry(8);
    expect(changeset.actor).toBe(author(server));
    expect(changeset.reason).toBe("the Bristol company");
    expect(changeset.ops).toEqual([{ op: "assign", record: waiting.id, from: null, to: other!.id }]);
  });

  test("is resolved by placing its record in a new identity", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const found = await server.client.reviewItem(1);
    open(server, "/review/1");
    await item();

    const dialog = await asked("New identity");
    expect((dialog.getByLabelText("crm:c-117") as HTMLSelectElement).value).toBe("new");
    await make("New identity");
    expect(await said()).toBe("1 record assigned at log position 8.");
    await waitFor(() => expect(field("Status")).toBe("resolved"));

    const { identity } = await server.client.record(waiting.id);
    expect(identity).toBeTruthy();
    expect(found.identities).not.toContain(identity!);
    expect((await entry(8)).changeset.ops).toEqual([
      { op: "create_identity", entity_type: "company", identity: identity! },
      { op: "assign", record: waiting.id, from: null, to: identity! },
    ]);
  });

  /** The two identities the item matched, the one with the most records first: it stands for the other once they are one. */
  async function matched() {
    const found = await server.client.reviewCase(1);
    const [into, other] = [...found.candidates].sort((a, b) => b.identity.members - a.identity.members);
    expect(into!.identity.members).toBeGreaterThan(other!.identity.members);
    return { found, into: into!.identity, other: other!.identity };
  }

  /** Takes back the decision that the two identities are not one, as a reviewer does on the screen of the pair. */
  async function allow() {
    const { found } = await matched();
    for (const { kind, a, b } of found.constraints) {
      await server.client.commit({
        reason: "one company after all",
        ops: [{ op: "unconstrain", kind, a, b }],
      });
    }
    return head();
  }

  const merging = () => screen.queryAllByRole("button", { name: "Merge identities" }).length;

  test("says what was decided about the identities it matched, and does not offer what that rules out", async () => {
    const { found, into, other } = await matched();
    // A reviewer split the two apart before the record arrived.
    const [kept] = found.constraints;
    expect(found.constraints).toHaveLength(1);
    expect(kept).toMatchObject({ kind: "cannot_link" });
    expect([...kept!.identities].sort()).toEqual([into.id, other.id].sort());
    const name = async (id: string) => {
      const record = await server.client.record(id);
      return `${record.source}:${record.key}`;
    };
    open(server, "/review/1");
    const card = await item();

    // It is said under each of the two, with the changeset that decided it and the way to why.
    const line = `cannot link${await name(kept!.a)} and ${await name(kept!.b)}, since log position ${kept!.from}explain`;
    await waitFor(() => expect(parts("constraint")).toEqual([line, line]));
    const decided = document.querySelector("tr[data-part=decided]")!;
    expect(decided.querySelector("th")!.textContent).toBe("Constraints");
    expect(parts("constraint", decided.querySelector("td")!)).toEqual([]);
    expect(decided.querySelector(`a[href='/log/${kept!.from}']`)).toBeTruthy();
    expect(decided.querySelector("[data-part=constraint] a[href^='/explain']")!.getAttribute("href")).toBe(
      `/explain?a=${kept!.a}&b=${kept!.b}`,
    );

    // The engine would refuse the merge, so it is not offered, and the item says why.
    expect(merging()).toBe(0);
    expect(parts("apart")).toHaveLength(1);
    // The record can still go to either, or to an identity of its own.
    expect(card.getAllByRole("button").map((button) => button.textContent)).toEqual([
      "New identity",
      ...found.candidates.map((candidate) => `Assign to ${candidate.identity.label}`),
      "Dismiss",
    ]);
    expect(changes()).toEqual([]);
  });

  test("does not offer an identity that its record is kept out of", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const { found, into, other } = await matched();
    const member = found.candidates.find((candidate) => candidate.identity.id === other.id)!.members[0]!;
    await server.client.commit({
      reason: "not the Bristol company",
      ops: [{ op: "constrain", kind: "cannot_link", a: waiting.id, b: member.id }],
    });
    calls.length = 0;
    open(server, "/review/1");
    const card = await item();

    await waitFor(() => expect(parts("constraint")).toHaveLength(3));
    expect(card.queryAllByRole("button", { name: `Assign to ${other.label}` })).toHaveLength(0);
    expect(card.getByText("Blocked by a constraint")).toBeTruthy();
    // Nor is it a choice where the record is placed from another button.
    const dialog = await asked(`Assign to ${into.label}`);
    const where = dialog.getByLabelText("crm:c-117") as HTMLSelectElement;
    expect([...where.options].map((option) => option.value)).toEqual(["", into.id, "new"]);
    expect(where.value).toBe(into.id);
    await make(`Assign to ${into.label}`);
    expect(await said()).toBe("1 record assigned at log position 9.");
    expect((await server.client.record(waiting.id)).identity).toBe(into.id);
  });

  test("is resolved by saying the identities it matched are one", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const { into, other } = await matched();
    open(server, "/review/1");
    await item();
    expect(merging()).toBe(0);

    // Once the decision that kept them apart is taken back, the screen read again offers the merge.
    expect(await allow()).toBe(8);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByRole("button", { name: "Merge identities" });
    await idle();
    expect(parts("constraint")).toEqual([]);
    expect(parts("apart")).toEqual([]);

    const dialog = await asked("Merge identities");
    expect(dialog.getByRole("heading").textContent).toBe("Merge 2 identities");
    // The dialog names what is merged into what.
    const about = screen.getByRole("dialog").textContent!;
    expect(about.indexOf(other.label!)).toBeGreaterThan(-1);
    expect(about.indexOf(into.label!)).toBeGreaterThan(about.indexOf(other.label!));
    expect(changes()).toEqual([]);

    await make("Merge identities", { Reason: "one company, two registrations" });
    expect(await said()).toBe("2 identities merged and 1 record assigned at log position 9.");
    await waitFor(() => expect(field("Status")).toBe("resolved"));

    // One changeset, written against what the screen read, that the log can undo.
    expect((await entry(9)).changeset).toMatchObject({
      actor: author(server),
      base: 8,
      reason: "one company, two registrations",
      ops: [
        { op: "merge", from: other.id, into: into.id },
        { op: "assign", record: waiting.id, from: null, to: into.id },
      ],
    });
    expect((await server.client.record(waiting.id)).identity).toBe(into.id);
    const merged = await server.client.identity(other.id);
    expect(merged.current).toBe(into.id);
    expect(merged.members).toHaveLength(into.members + other.members + 1);
  });

  test("does not merge identities that changed after the screen was read", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const moved = await row(server.client, "crm", "c-108");
    const { into } = await matched();
    expect(await allow()).toBe(8);
    open(server, "/review/1");
    await item();

    // Someone gives one of them another record while the item is read.
    await server.client.commit({
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: into.id }],
    });

    await make("Merge identities");
    const answer = await refused();
    expect(answer).toContain(
      `stale merge: the members of identity ${into.id} changed at log position 9, after the changeset's base 8`,
    );
    expect(answer).toContain("Nothing was changed.");
    expect((await server.client.record(waiting.id)).identity ?? null).toBeNull();
    expect(await head()).toBe(9);

    // Read again, the item shows what the identity holds now, and the merge is made of that.
    await cancel();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("3 records");
    await idle();
    await make("Merge identities");
    expect(await said()).toBe("2 identities merged and 1 record assigned at log position 10.");
    expect((await entry(10)).changeset).toMatchObject({ base: 9 });
  });

  test("is merged against what its card showed, though the store changed before the card was read", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const moved = await row(server.client, "crm", "c-108");
    const { into } = await matched();
    expect(await allow()).toBe(8);
    // The studio is open, and then someone gives one of the identities another record.
    open(server, "");
    await heading("Overview");
    await idle();
    await server.client.commit({
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: into.id }],
    });

    // The reviewer goes to the items without reading the studio again. The card shows
    // the identity as it is now, so the merge is of what they saw.
    follow(screens().getByRole("link", { name: /^Review/ }));
    await heading("Review");
    await screen.findByRole("article", { name: "Review item 1" });
    await screen.findByText("3 records");
    await idle();
    await make("Merge identities");
    expect(await said()).toBe("2 identities merged and 1 record assigned at log position 10.");
    expect((await entry(10)).changeset).toMatchObject({ base: 9 });
    expect((await server.client.record(waiting.id)).identity).toBe(into.id);
  });

  test("says what the run found against every record of an identity, those it does not show too", async () => {
    const { into } = await matched();
    // The identity gains more records than a card shows, each with the name of the one that waits.
    const rows = Array.from({ length: 30 }, (_, n) => ({
      id: `c-9${String(n).padStart(2, "0")}`,
      name: "Pioneer Supplies",
      country: "GB",
    }));
    const ingested = await server.client.ingest({ source: "crm", entity_type: "company", rows });
    expect(ingested.status).toBe("succeeded");
    const added = await Promise.all(rows.map(({ id }) => row(server.client, "crm", id)));
    await server.client.commit({
      reason: "branches of the Leeds company",
      ops: added.map((record) => ({ op: "assign", record: record.id, from: null, to: into.id })),
    });
    await server.client.startRun({ entity_type: "company" });

    const found = await server.client.reviewCase(1);
    expect(found.item.status).toBe("open");
    const shown = found.candidates.find((candidate) => candidate.identity.id === into.id)!.members;
    const against = found.judgements.filter((judgement) => judgement.identity === into.id);
    expect(shown).toHaveLength(25);
    expect(against).toHaveLength(32);

    open(server, "/review/1");
    await item();
    await screen.findByText("32 records");
    const column = found.candidates.findIndex((candidate) => candidate.identity.id === into.id);
    // The card says that it compares some of the identity's records, and how many.
    expect(parts("sample")).toEqual(["25 shown"]);
    // A field in which they all differ is not written out value by value.
    const ids = new Set(shown.flatMap((member) => (member.payload as { id?: string }).id ?? []));
    expect(ids.size).toBeGreaterThan(20);
    const held = document.querySelectorAll("tr[data-field=id] td")[found.records.length + column]!;
    expect(held.querySelectorAll("[data-agrees]")).toHaveLength(4);
    expect(parts("more", held)).toEqual([`+${ids.size - 4} more`]);

    // Under the identity, the first of them are written out and the rest are counted: none is left out.
    const cell = () => document.querySelectorAll("tr[data-part=found] td")[found.records.length + column]!;
    expect(parts("judgement", cell())).toHaveLength(6);
    expect(parts("more", cell())).toEqual(["+26 more: 26 match"]);
    // Asked for, the rest are written out too, each named though the case did not hold its record.
    fireEvent.click(within(cell() as HTMLElement).getByRole("button", { name: "Show all 32" }));
    expect(parts("judgement", cell())).toHaveLength(32);
    expect(parts("more", cell())).toEqual([]);
    const beyond = against.filter((judgement) => shown.every((member) => member.id !== judgement.other));
    expect(beyond).toHaveLength(7);
    await waitFor(() =>
      expect(
        texts("[data-part=judgement] a[data-kind=record]", cell()).every((name) => /^crm:c-|^registry:X/.test(name)),
      ).toBe(true),
    );
    expect(changes()).toEqual([]);
  });

  test("is decided from the list of those that wait, and leaves it", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const found = await server.client.reviewCase(1);
    const [first] = found.candidates.map((candidate) => candidate.identity);
    open(server, "/review");
    await heading("Review");
    await screen.findByRole("article", { name: "Review item 1" });
    await idle();

    await make(`Assign to ${first!.label}`);
    // What came of it is said where the item was.
    expect(await said()).toBe("1 record assigned at log position 8.");
    await waitFor(() => expect(screen.queryByRole("article")).toBeNull());
    expect(parts("empty")[0]).toStartWith("No open review items.");
    expect(document.querySelector("[role=status] a")?.getAttribute("href")).toBe("/log/8");
    await waitFor(() => expect(screens().getByRole("link", { name: /^Review/ }).textContent).toBe("Review"));
    expect((await server.client.record(waiting.id)).identity).toBe(first!.id);
    expect((await server.client.reviewItem(1)).status).toBe("resolved");
  });

  test("is not changed until the reviewer says where its record goes", async () => {
    open(server, "/review/1");
    await item();

    await make("New identity", { "crm:c-117": "" });
    await waitFor(async () => expect(await refused()).toBe("Choose an identity for at least one record."));

    expect(changes()).toEqual([]);
    expect(await head()).toBe(7);

    // Leaving the dialog leaves the item as it was.
    await cancel();
    expect(field("Status")).toBe("open");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("is dismissed under the reviewer's name", async () => {
    open(server, "/review/1");
    const card = await item();

    await make("Dismiss", { Reason: "not a company of ours" });
    expect(await said()).toBe("Dismissed.");
    await waitFor(() => expect(field("Status")).toBe("dismissed"));
    expect(field("Dismissed by")).toMatch(/^Ada at \S+ \S+ “not a company of ours”$/);
    expect(card.queryAllByRole("button")).toEqual([]);

    const dismissed = await server.client.reviewItem(1);
    expect(dismissed.status).toBe("dismissed");
    expect(dismissed.dismissed?.by).toBe(author(server));
    expect(changes()).toEqual(["POST /v1/review/1/dismiss"]);
  });

  test("that someone else settled first says so", async () => {
    open(server, "/review/1");
    await item();
    await server.client.dismissReview(1, {});

    await make("Dismiss");
    expect(await refused()).toMatch(/^review item 1: /);
    expect(screen.queryByRole("status")).toBeNull();
  });
});
