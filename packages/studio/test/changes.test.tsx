// What a reviewer changes from the studio, against the real server: each
// change is made on a screen, and the server is asked what came of it.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";

import type { Server } from "../../client/test/server.ts";
import { REVIEWER_KEY } from "../src/app.tsx";
import {
  SEEDING,
  calls,
  counted,
  field,
  follow,
  heading,
  idle,
  memory,
  open,
  parts,
  screens,
  start,
  stopAll,
  texts,
} from "./harness.tsx";
import { REVIEWER, companies, row, seedCompanies } from "./seed.ts";

let server: Server;

// Every test changes what the store holds, so each has a store of its own.
beforeEach(async () => {
  server = await start(companies, 7);
  await seedCompanies(server.client);
  calls.length = 0;
}, SEEDING);

afterEach(async () => {
  cleanup();
  window.history.replaceState(null, "", "/");
  await stopAll();
});

// A change is proposed by a button, and made from the dialog the button
// opens: it says what the change would do, and takes the reviewer's name and
// their reason.

/**
 * Whether a dialog is open. Asked as a yes or no: an element that an
 * expectation fails on is written out whole, and a page is long.
 */
const asking = () => screen.queryByRole("dialog") !== null;

/** The dialog a button opens. It is opened unless one already is. */
async function asked(button: string | RegExp) {
  if (!asking()) fireEvent.click(screen.getByRole("button", { name: button }));
  return within(await screen.findByRole("dialog"));
}

/** Fills the fields of a button's dialog by their labels. */
async function fill(button: string | RegExp, fields: Record<string, string>) {
  const dialog = await asked(button);
  for (const [label, value] of Object.entries(fields)) {
    fireEvent.change(dialog.getByLabelText(label), { target: { value } });
  }
  return dialog;
}

/** Fills a button's dialog and presses the button in it that makes the change. */
async function make(button: string | RegExp, fields: Record<string, string> = {}) {
  const dialog = await fill(button, fields);
  fireEvent.click(dialog.getByRole("button", { name: button }));
}

/** Leaves a dialog without making its change. */
async function cancel() {
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(asking()).toBe(false));
}

/** Looks up the identity that another is merged with, on the screen of that other. */
function lookUp(reference?: string) {
  if (reference !== undefined) {
    fireEvent.change(screen.getByLabelText("Identity to merge into this one"), { target: { value: reference } });
  }
  fireEvent.click(screen.getByRole("button", { name: "Find" }));
}

/** What the screen says came of the last change. */
async function said(): Promise<string | null> {
  // A change that was made closes its dialog, and the screen says what came of it.
  await waitFor(() => expect(asking()).toBe(false));
  const status = await screen.findByRole("status");
  await idle();
  return status.textContent;
}

/** Why the screen says a change was not made. */
async function refused(): Promise<string> {
  const alert = await screen.findByRole("alert");
  await idle();
  return alert.textContent ?? "";
}

/** The calls the studio made that were not reads. */
const changes = () => calls.filter((call) => !call.startsWith("GET "));

const head = async () => (await server.client.info()).store.head;

const entry = async (seq: number) => (await server.client.changesets({ after: seq - 1, limit: 1 })).changesets[0]!;

const ada = { "Your name": "ada" };

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
      actor: REVIEWER,
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
    const names = memory();
    open(server, "/review/1", undefined, names);
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

    await make(`Assign to ${other!.label}`, { ...ada, Reason: "the Bristol company" });
    expect(await said()).toBe("1 record assigned at log position 8.");
    await waitFor(() => expect(field("Status")).toBe("resolved"));
    expect(field("Resolved at")).toBe("log position 8");
    // It is decided, so there is nothing left to press.
    expect(card.queryAllByRole("button")).toEqual([]);
    await waitFor(() => expect(screens().getByRole("link", { name: /^Review/ }).textContent).toBe("Review"));

    expect((await server.client.record(waiting.id)).identity).toBe(other!.id);
    const { changeset } = await entry(8);
    expect(changeset.actor).toBe("user:ada");
    expect(changeset.reason).toBe("the Bristol company");
    expect(changeset.ops).toEqual([{ op: "assign", record: waiting.id, from: null, to: other!.id }]);
    // The name is kept for the next change, in this tab or another.
    expect(names.getItem(REVIEWER_KEY)).toBe("ada");
  });

  test("is resolved by placing its record in a new identity", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const found = await server.client.reviewItem(1);
    open(server, "/review/1");
    await item();

    const dialog = await asked("New identity");
    expect((dialog.getByLabelText("crm:c-117") as HTMLSelectElement).value).toBe("new");
    await make("New identity", ada);
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
        actor: REVIEWER,
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
      actor: REVIEWER,
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
    await make(`Assign to ${into.label}`, ada);
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

    await make("Merge identities", { ...ada, Reason: "one company, two registrations" });
    expect(await said()).toBe("2 identities merged and 1 record assigned at log position 9.");
    await waitFor(() => expect(field("Status")).toBe("resolved"));

    // One changeset, written against what the screen read, that the log can undo.
    expect((await entry(9)).changeset).toMatchObject({
      actor: "user:ada",
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
      actor: REVIEWER,
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: into.id }],
    });

    await make("Merge identities", ada);
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
    await make("Merge identities", ada);
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
      actor: REVIEWER,
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: into.id }],
    });

    // The reviewer goes to the items without reading the studio again. The card shows
    // the identity as it is now, so the merge is of what they saw.
    follow(screens().getByRole("link", { name: /^Review/ }));
    await heading("Review");
    await screen.findByRole("article", { name: "Review item 1" });
    await screen.findByText("3 records");
    await idle();
    await make("Merge identities", ada);
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
    const ingested = await server.client.ingest({ source: "crm", entity_type: "company", key: "id", rows });
    expect(ingested.status).toBe("succeeded");
    const added = await Promise.all(rows.map(({ id }) => row(server.client, "crm", id)));
    await server.client.commit({
      actor: REVIEWER,
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

    await make(`Assign to ${first!.label}`, ada);
    // What came of it is said where the item was.
    expect(await said()).toBe("1 record assigned at log position 8.");
    await waitFor(() => expect(screen.queryByRole("article")).toBeNull());
    expect(parts("empty")[0]).toStartWith("No open review items.");
    expect(document.querySelector("[role=status] a")?.getAttribute("href")).toBe("/log/8");
    await waitFor(() => expect(screens().getByRole("link", { name: /^Review/ }).textContent).toBe("Review"));
    expect((await server.client.record(waiting.id)).identity).toBe(first!.id);
    expect((await server.client.reviewItem(1)).status).toBe("resolved");
  });

  test("is not changed until the reviewer says who they are and where its record goes", async () => {
    open(server, "/review/1");
    await item();

    await make("New identity");
    expect(await refused()).toBe("Enter your name.");
    await make("New identity", { ...ada, "crm:c-117": "" });
    await waitFor(async () => expect(await refused()).toBe("Choose an identity for at least one record."));

    expect(changes()).toEqual([]);
    expect(await head()).toBe(7);

    // Leaving the dialog leaves the item as it was.
    await cancel();
    expect(field("Status")).toBe("open");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("is dismissed under the reviewer's name", async () => {
    open(server, "/review/1", undefined, memory());
    const card = await item();

    await make("Dismiss", { ...ada, Reason: "not a company of ours" });
    expect(await said()).toBe("Dismissed.");
    await waitFor(() => expect(field("Status")).toBe("dismissed"));
    expect(field("Dismissed by")).toMatch(/^user:ada at \S+ \S+ “not a company of ours”$/);
    expect(card.queryAllByRole("button")).toEqual([]);

    const dismissed = await server.client.reviewItem(1);
    expect(dismissed.status).toBe("dismissed");
    expect(dismissed.dismissed?.by).toBe("user:ada");
    expect(changes()).toEqual(["POST /v1/review/1/dismiss"]);
  });

  test("that someone else settled first says so", async () => {
    open(server, "/review/1");
    await item();
    await server.client.dismissReview(1, { actor: REVIEWER });

    await make("Dismiss", ada);
    expect(await refused()).toMatch(/^review item 1: /);
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("a record", () => {
  test("is moved to another identity, to one of its own, and out of any", async () => {
    const moved = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    const names = memory();
    names.setItem(REVIEWER_KEY, "grace");
    open(server, `/records/${moved.id}`, undefined, names);
    await heading("Atlas Trading");
    await screen.findByRole("heading", { name: "Move" });
    await idle();
    // The name the reviewer last used is filled in.
    expect(((await asked("Move")).getByLabelText("Your name") as HTMLInputElement).value).toBe("grace");

    // The identity it goes to can be named by a record it holds.
    await make("Move", { Identity: "crm:nothing" });
    expect(await refused()).toBe("No identity found for crm:nothing.");
    await make("Move", { Identity: "crm:c-109", Reason: "one company after all" });
    expect(await said()).toBe("Moved at log position 8.");
    await waitFor(() =>
      expect(document.querySelector("dd a[data-kind=identity]")?.getAttribute("title")).toBe(other.identity!),
    );
    // The identity it is in now is named by what its records call it.
    await waitFor(() => expect(field("Identity")).toBe(`Atlas Trading${other.identity!.slice(-4)}`));
    expect((await entry(8)).changeset).toMatchObject({
      actor: "user:grace",
      reason: "one company after all",
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: other.identity! }],
    });

    await make("Move", { To: "new" });
    await waitFor(async () => expect(await said()).toBe("Moved at log position 9."));
    const alone = (await server.client.record(moved.id)).identity!;
    expect([moved.identity, other.identity]).not.toContain(alone);
    await waitFor(() => expect(document.querySelector("dd a[data-kind=identity]")?.getAttribute("title")).toBe(alone));

    await make("Move", { To: "none" });
    await waitFor(async () => expect(await said()).toBe("Unassigned at log position 10."));
    await waitFor(() => expect(field("Identity")).toBe("unassigned"));
    expect((await server.client.record(moved.id)).identity ?? null).toBeNull();
    // It has no identity to be taken out of now, so it can only be placed.
    await screen.findByRole("button", { name: "Assign" });
    const where = (await asked("Assign")).getByLabelText("To") as HTMLSelectElement;
    expect([...where.options].map((option) => option.value)).toEqual(["identity", "new"]);
  });

  test("that was moved by someone else since it was read is left where it is", async () => {
    const moved = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    open(server, `/records/${moved.id}`);
    await heading("Atlas Trading");
    await screen.findByRole("heading", { name: "Move" });
    await idle();
    await server.client.commit({
      actor: REVIEWER,
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: other.identity! }],
    });

    await make("Move", { ...ada, To: "none" });
    const answer = await refused();
    expect(answer).toContain(`stale precondition: record ${moved.id} was expected to belong to ${moved.identity}`);
    expect(answer).toContain("Nothing was changed.");
    expect((await server.client.record(moved.id)).identity).toBe(other.identity!);
    expect(await head()).toBe(8);
  });
});

describe("an identity", () => {
  test("is split in two by ticking the members that are something else", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const other = await row(server.client, "crm", "c-122");
    open(server, `/identities/${orchard.identity}`);
    await counted("Records", 3);
    await idle();
    expect(screen.queryByRole("button", { name: /^Split the/ })).toBeNull();

    // The reviewer said crm:c-111 and registry:X0000011 are one company, so
    // the engine does not part them.
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${orchard.id}` }));
    await make("Split", ada);
    const answer = await refused();
    expect(answer).toContain("have a must_link constraint, and would belong to identities");
    expect(answer).toContain("Nothing was changed.");
    expect(await head()).toBe(7);

    await cancel();
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${orchard.id}` }));
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${other.id}` }));
    await make("Split", { Reason: "Orchard Garden is another company" });
    expect(await said()).toMatch(/^Split at log position 8: 1 record moved to 0\S+\.$/);
    await counted("Records", 2);

    const parted = (await server.client.record(other.id)).identity!;
    expect(parted).not.toBe(orchard.identity!);
    expect(document.querySelector("[role=status] a[href^='/identities/']")?.getAttribute("href")).toBe(
      `/identities/${parted}`,
    );
    expect((await server.client.identity(orchard.identity!)).members.map((member) => member.key).sort()).toEqual([
      "X0000011",
      "c-111",
    ]);
    // The two sides are kept apart unless the reviewer says otherwise.
    expect(await server.client.constraints({ record: other.id })).toMatchObject([{ kind: "cannot_link", from: 8 }]);
    expect((await entry(8)).changeset).toMatchObject({
      actor: "user:ada",
      reason: "Orchard Garden is another company",
    });
  });

  test("is split without keeping the sides apart when the reviewer unticks that", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const other = await row(server.client, "crm", "c-122");
    open(server, `/identities/${orchard.identity}`);
    await counted("Records", 3);
    await idle();

    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${other.id}` }));
    const dialog = await asked("Split");
    // What the split would do is said again where it is made.
    expect(dialog.getByText("1 selected record moves to a new identity. The other 2 keep this ID.")).toBeTruthy();
    const apart = dialog.getByRole("checkbox", { name: "Keep the two sides apart in later runs" });
    expect(apart.getAttribute("aria-checked")).toBe("true");
    // The words are pressed, as a label is: where these tests run, a press on the box itself
    // counts twice, which a browser does not do.
    fireEvent.click(dialog.getByText("Keep the two sides apart in later runs"));
    expect(apart.getAttribute("aria-checked")).toBe("false");
    await make("Split", ada);
    expect(await said()).toMatch(/^Split at log position 8: /);
    await counted("Records", 2);
    expect(await server.client.constraints({ record: other.id })).toEqual([]);
  });

  test("takes in another identity once the reviewer has seen what it holds", async () => {
    const atlas = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    open(server, `/identities/${atlas.identity}`);
    await counted("Records", 2);
    await idle();
    expect(screen.queryByRole("button", { name: "Merge" })).toBeNull();

    lookUp("crm:nothing");
    expect(await refused()).toBe("No identity found for crm:nothing.");
    lookUp(atlas.identity!);
    await waitFor(async () => expect(await refused()).toBe("That is this identity."));

    lookUp("crm:c-109");
    await screen.findByRole("button", { name: "Merge" });
    expect(screen.getByText(/· 2 records:/).textContent).toBe(
      "Atlas Trading · 2 records: crm:c-109, registry:X0512345",
    );
    expect(changes()).toEqual([]);

    await make("Merge", { ...ada, Reason: "one company, two offices" });
    expect(await said()).toBe("Merged at log position 8.");
    await counted("Records", 4);
    expect(screen.queryByRole("button", { name: "Merge" })).toBeNull();

    expect((await server.client.record(other.id)).identity).toBe(atlas.identity!);
    expect((await entry(8)).changeset).toMatchObject({
      actor: "user:ada",
      base: 7,
      reason: "one company, two offices",
      ops: [{ op: "merge", from: other.identity!, into: atlas.identity! }],
    });
  });

  test("is not merged when its members changed after the screen was read", async () => {
    const atlas = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    const moved = await row(server.client, "registry", "X0000010");
    open(server, `/identities/${atlas.identity}`);
    await counted("Records", 2);
    await idle();

    // Someone gives it another member while it is read.
    await server.client.commit({
      actor: REVIEWER,
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: atlas.identity! }],
    });

    lookUp(other.identity!);
    await screen.findByRole("button", { name: "Merge" });
    await make("Merge", ada);
    const answer = await refused();
    expect(answer).toContain(
      `stale merge: the members of identity ${atlas.identity} changed at log position 8, after the changeset's base 7`,
    );
    expect(answer).toContain("Nothing was changed.");
    expect((await server.client.record(other.id)).identity).toBe(other.identity!);

    // Read again, it shows what it holds now, and the merge is made of that.
    await cancel();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await counted("Records", 3);
    lookUp();
    await screen.findByRole("button", { name: "Merge" });
    await make("Merge", ada);
    expect(await said()).toBe("Merged at log position 9.");
    await counted("Records", 5);
  });

  test("says which side of a split stays, and it is the side that does", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const registered = await row(server.client, "registry", "X0000011");
    const other = await row(server.client, "crm", "c-122");
    open(server, `/identities/${orchard.identity}`);
    await counted("Records", 3);
    await idle();

    // The larger side keeps the ID, whichever side was ticked.
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${other.id}` }));
    screen.getByText("1 selected record moves to a new identity. The other 2 keep this ID.");
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${other.id}` }));
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${orchard.id}` }));
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${registered.id}` }));
    screen.getByText("2 selected records keep this ID. The other record moves to a new identity.");

    await make("Split", ada);
    expect(await said()).toMatch(/^Split at log position 8: 1 record moved to 0\S+\.$/);
    await counted("Records", 2);
    expect((await server.client.record(orchard.id)).identity).toBe(orchard.identity!);
    expect((await server.client.record(registered.id)).identity).toBe(orchard.identity!);
    expect((await server.client.record(other.id)).identity).not.toBe(orchard.identity!);
  });

  test("is not split when its members changed after the screen was read", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const registered = await row(server.client, "registry", "X0000011");
    const other = await row(server.client, "crm", "c-122");
    const added = await row(server.client, "registry", "X0000010");
    open(server, `/identities/${orchard.identity}`);
    await counted("Records", 3);
    await idle();
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${orchard.id}` }));
    fireEvent.click(screen.getByRole("checkbox", { name: `Select record ${registered.id}` }));
    screen.getByText(/^2 selected records keep this ID/);

    // Someone gives it a fourth member, and the ticked are no longer the
    // larger side: the engine would move them, and not the other one.
    await server.client.commit({
      actor: REVIEWER,
      ops: [{ op: "assign", record: added.id, from: added.identity!, to: orchard.identity! }],
    });

    await make("Split", ada);
    expect(await refused()).toBe(
      `identity ${orchard.identity} cannot be split: its members changed at log position 8, after log position 7 that the split is based on`,
    );
    expect(await head()).toBe(8);
    expect((await server.client.identity(orchard.identity!)).members).toHaveLength(4);

    // Read again, it says what a split of what it holds now would do.
    await cancel();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await counted("Records", 4);
    screen.getByText("2 selected records move to a new identity. The other 2 keep this ID.");
    await make("Split", ada);
    expect(await said()).toMatch(/^Split at log position 9: 2 records moved to 0\S+\.$/);
    expect((await server.client.record(other.id)).identity).toBe(orchard.identity!);
    expect((await server.client.record(orchard.id)).identity).not.toBe(orchard.identity!);
  });

  test("does not take in members the reviewer has not seen", async () => {
    const atlas = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    const moved = await row(server.client, "registry", "X0000010");
    open(server, `/identities/${atlas.identity}`);
    await counted("Records", 2);
    await idle();
    lookUp("crm:c-109");
    await screen.findByText(/· 2 records:/);
    await idle();

    // Someone gives the other identity a member after it was looked up.
    await server.client.commit({
      actor: REVIEWER,
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: other.identity! }],
    });

    // Reading this screen again moves on what a merge is written against,
    // so what was looked up before is no longer what would be merged.
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("The store changed since. Find it again.");
    await idle();
    expect(screen.queryByRole("button", { name: "Merge" })).toBeNull();
    expect(screen.queryByText(/· 2 records:/)).toBeNull();
    expect(changes()).toEqual([]);

    lookUp();
    expect((await screen.findByText(/· 3 records:/)).textContent).toContain(
      "crm:c-109, registry:X0000010, registry:X0512345",
    );
    await make("Merge", ada);
    expect(await said()).toBe("Merged at log position 9.");
    await counted("Records", 5);
    expect((await entry(9)).changeset).toMatchObject({ actor: "user:ada", base: 8 });
  });

  test("that was merged away offers no change", async () => {
    const { changeset } = await entry(6);
    const merge = changeset.ops.find((op) => op.op === "merge");
    if (merge?.op !== "merge") throw new Error("log position 6 has no merge");
    open(server, `/identities/${merge.from}`);
    await counted("Records", 3);
    await idle();
    expect(screen.queryByText("Split")).toBeNull();
    expect(screen.queryByText("Merge")).toBeNull();
  });
});

describe("two records", () => {
  test("are kept apart by a constraint, which can be removed again", async () => {
    const a = await row(server.client, "crm", "c-108");
    const b = await row(server.client, "crm", "c-109");
    open(server, `/explain?a=${a.id}&b=${b.id}`);
    await heading("Explain");
    await screen.findByText("Different identities");
    await idle();

    // They are apart, so a constraint that they are one would not hold.
    const kind = (await asked("Add constraint")).getByLabelText("Kind") as HTMLSelectElement;
    expect([...kind.options].map((option) => option.value)).toEqual(["cannot_link"]);

    await make("Add constraint", { ...ada, Reason: "two companies of one name" });
    expect(await said()).toBe("Added cannot_link at log position 8.");
    await screen.findByText("Constraints");
    await screen.findByRole("button", { name: "Remove cannot_link" });
    expect(screen.queryByRole("button", { name: "Add constraint" })).toBeNull();
    expect(await server.client.constraints({ record: a.id })).toMatchObject([{ kind: "cannot_link", from: 8 }]);
    expect((await entry(8)).changeset).toMatchObject({ actor: "user:ada", reason: "two companies of one name" });

    await make("Remove cannot_link");
    await waitFor(async () => expect(await said()).toBe("Removed cannot_link at log position 9."));
    await screen.findByRole("button", { name: "Add constraint" });
    expect(await server.client.constraints({ record: a.id })).toEqual([]);
  });

  test("of one identity can only be said to be one", async () => {
    const versions = (await server.client.records({ source: "crm", key: "c-104" })).records;
    open(server, `/explain?a=${versions[0]!.id}&b=${versions[1]!.id}`);
    await heading("Explain");
    await screen.findByText("Same identity");
    await idle();
    const kind = (await asked("Add constraint")).getByLabelText("Kind") as HTMLSelectElement;
    expect([...kind.options].map((option) => option.value)).toEqual(["must_link"]);
  });

  test("without an identity can be said to be one or to be two", async () => {
    const a = await row(server.client, "crm", "c-117");
    const b = await row(server.client, "registry", "X0000009");
    open(server, `/explain?a=${a.id}&b=${b.id}`);
    await heading("Explain");
    await screen.findByText("Not both assigned");
    await idle();

    await make("Add constraint", { ...ada, Kind: "must_link" });
    expect(await said()).toBe("Added must_link at log position 8.");
    await screen.findByRole("button", { name: "Remove must_link" });
    expect(await server.client.constraints({ record: a.id })).toMatchObject([{ kind: "must_link", from: 8 }]);
  });
});

describe("a changeset in the log", () => {
  test("is undone after the screen showed what undoing it would do", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    open(server, "/log/6");
    await heading("Log position 6");
    await counted("Operations", 2);
    await idle();

    // The dialog that undoes it can show what that would do first.
    const dialog = await fill("Undo", ada);
    fireEvent.click(dialog.getByRole("button", { name: "Preview" }));
    await screen.findByText("Would commit:");
    await waitFor(() =>
      expect(texts("[data-part=looked] [data-part=ops] li")).toEqual([
        expect.stringMatching(/^remove must_link between crm:c-111 and registry:X0000011$/),
        // The identity that comes back holds no record yet, so nothing calls it anything.
        expect.stringMatching(/^reinstate identity 0\S+$/),
        expect.stringMatching(/^move record crm:c-111 from .+ \(…[0-9a-f]{4}\) to 0\S+$/),
      ]),
    );
    // Looking made a call, and changed nothing.
    expect(changes()).toEqual(["POST /v1/changesets/6/undo"]);
    expect(await head()).toBe(7);
    expect(screen.queryByRole("status")).toBeNull();

    await make("Undo", { Reason: "Orchard is not Orchard Foods" });
    expect(await said()).toBe("Undone at log position 8.");
    expect(await head()).toBe(8);
    expect((await entry(8)).changeset).toMatchObject({ actor: "user:ada", reason: "Orchard is not Orchard Foods" });
    expect((await server.client.record(orchard.id)).identity).not.toBe(orchard.identity!);
    expect(await server.client.constraints({ record: orchard.id })).toEqual([]);
  });

  test("that was undone before is not undone again", async () => {
    open(server, "/log/2");
    await heading("Log position 2");
    await counted("Operations", 1);
    await idle();

    await make("Undo", ada);
    const answer = await refused();
    expect(answer).toMatch(/^changeset rejected: operation 0 \(reinstate\): identity \S+ is active/);
    expect(answer).toContain("Nothing was changed.");
    expect(texts("[role=alert] li")).toEqual([expect.stringMatching(/^Operation 0: reinstate identity 0/)]);
    expect(await head()).toBe(7);
  });
});
