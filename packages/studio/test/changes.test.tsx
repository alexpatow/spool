// What a reviewer changes from the studio, against the real server: each
// change is made on a screen, and the server is asked what came of it.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";

import type { Server } from "../../client/test/server.ts";
import { REVIEWER_KEY } from "../src/app.tsx";
import { SEEDING, calls, field, heading, idle, memory, open, start, stopAll, texts } from "./harness.tsx";
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
  window.location.hash = "";
  await stopAll();
});

/** The form a button belongs to. */
function formOf(button: string | RegExp) {
  const form = screen.getByRole("button", { name: button }).closest("form");
  if (form === null) throw new Error(`the button ${button} is in no form`);
  return within(form);
}

/** Fills the fields of a form by their labels. */
function fill(button: string | RegExp, fields: Record<string, string>) {
  const form = formOf(button);
  for (const [label, value] of Object.entries(fields)) {
    fireEvent.change(form.getByLabelText(label), { target: { value } });
  }
}

/** Fills a form and presses the button that makes the change. */
function make(button: string | RegExp, fields: Record<string, string> = {}) {
  fill(button, fields);
  fireEvent.click(screen.getByRole("button", { name: button }));
}

/** What the screen says came of the last change. */
async function said(): Promise<string | null> {
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
    open(server, `#/drafts/${draft!.id}`);
    await heading("Draft");
    await screen.findByText("Operations (5)");
    await idle();

    // Pressed twice before the screen has shown the first press, it is made once.
    const form = screen.getByRole("button", { name: "Approve and commit" }).closest("form")!;
    act(() => {
      fireEvent.submit(form);
      fireEvent.submit(form);
    });
    expect(await said()).toBe("The draft was committed at log position 8.");
    expect(screen.queryByRole("alert")).toBeNull();
    await waitFor(() => expect(field("Status")).toBe("committed"));
    expect(field("Committed at")).toBe("log position 8");
    // It is decided, so there is nothing left to press.
    expect(screen.queryByRole("button", { name: "Approve and commit" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Reject" })).toBeNull();
    await waitFor(() =>
      expect(within(screen.getByRole("navigation")).getByRole("link", { name: /^Drafts/ }).textContent).toBe("Drafts"),
    );

    expect((await server.client.draft(draft!.id)).draft.status).toBe("committed");
    expect(await head()).toBe(8);
    expect(changes()).toEqual([`POST /v1/drafts/${draft!.id}/approve`]);
  });

  test("that no longer holds is not committed, and the screen names the operations", async () => {
    const [draft] = await server.client.drafts({ status: "open" });
    const { changeset } = await server.client.draft(draft!.id);
    const placed = changeset.ops.find((op) => op.op === "assign");
    if (placed?.op !== "assign") throw new Error("the draft places nothing");
    const name = (await server.client.observation(placed.observation)).key;
    open(server, `#/drafts/${draft!.id}`);
    await heading("Draft");
    await screen.findByText("Operations (5)");
    await idle();

    // Someone places one of its observations elsewhere while it is read.
    const [elsewhere] = (await server.client.mintIds({ identities: 1 })).identities;
    await server.client.commit({
      actor: REVIEWER,
      ops: [
        { op: "create_identity", entity_type: "company", identity: elsewhere! },
        { op: "assign", observation: placed.observation, from: null, to: elsewhere! },
      ],
    });

    make("Approve and commit");
    const answer = await refused();
    expect(answer).toContain(
      `changeset rejected: operation 0 (assign): stale precondition: observation ${placed.observation} was expected to belong to no identity`,
    );
    expect(answer).toContain("Nothing was changed.");
    // The operation the engine names is shown as the list of them shows it.
    expect(texts("[role=alert] li")).toHaveLength(1);
    expect(texts("[role=alert] li")[0]).toMatch(/^Operation 0: place observation \S+ in 0/);
    await waitFor(() => expect(texts(".ops li")[0]).toStartWith(`place observation registry:${name} in 0`));

    expect(screen.queryByRole("status")).toBeNull();
    expect(field("Status")).toBe("open");
    expect((await server.client.draft(draft!.id)).draft.status).toBe("open");
    expect(await head()).toBe(8);
  });

  test("is rejected with the reason the reviewer gave", async () => {
    const [draft] = await server.client.drafts({ status: "open" });
    open(server, `#/drafts/${draft!.id}`);
    await heading("Draft");
    await screen.findByText("Operations (5)");
    await idle();

    make("Reject", { "Why it is rejected": " the Leeds record is not ours " });
    expect(await said()).toBe("The draft was rejected. The next run examines its observations again.");
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
  test("is resolved by placing its observation in an identity it matched", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const item = await server.client.reviewItem(1);
    const names = memory();
    open(server, "#/review/1", undefined, names);
    await heading("Review item 1");
    await screen.findByText("Identities they matched");
    await idle();

    // Where it may go: nowhere, an identity it matched, or one of its own.
    const where = formOf("Place it").getByLabelText("Where crm:c-117 goes") as HTMLSelectElement;
    expect([...where.options].map((option) => option.value)).toEqual(["", ...item.identities, "new"]);

    make("Place it", { ...ada, Reason: "the Leeds office of Atlas", "Where crm:c-117 goes": item.identities[1]! });
    expect(await said()).toBe("1 observation placed at log position 8.");
    await waitFor(() => expect(field("Status")).toBe("resolved"));
    expect(field("Resolved at")).toBe("log position 8");
    expect(screen.queryByRole("button", { name: "Place it" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Dismiss" })).toBeNull();
    await waitFor(() =>
      expect(within(screen.getByRole("navigation")).getByRole("link", { name: /^Review/ }).textContent).toBe("Review"),
    );

    expect((await server.client.observation(waiting.id)).identity).toBe(item.identities[1]!);
    const { changeset } = await entry(8);
    expect(changeset.actor).toBe("user:ada");
    expect(changeset.reason).toBe("the Leeds office of Atlas");
    expect(changeset.ops).toEqual([{ op: "assign", observation: waiting.id, from: null, to: item.identities[1]! }]);
    // The name is kept for the next change, in this tab or another.
    expect(names.getItem(REVIEWER_KEY)).toBe("ada");
  });

  test("is resolved by placing its observation in a new identity", async () => {
    const waiting = await row(server.client, "crm", "c-117");
    const item = await server.client.reviewItem(1);
    open(server, "#/review/1");
    await heading("Review item 1");
    await screen.findByText("Identities they matched");
    await idle();

    make("Place it", { ...ada, "Where crm:c-117 goes": "new" });
    expect(await said()).toBe("1 observation placed at log position 8.");
    await waitFor(() => expect(field("Status")).toBe("resolved"));

    const { identity } = await server.client.observation(waiting.id);
    expect(identity).toBeTruthy();
    expect(item.identities).not.toContain(identity!);
    expect((await entry(8)).changeset.ops).toEqual([
      { op: "create_identity", entity_type: "company", identity: identity! },
      { op: "assign", observation: waiting.id, from: null, to: identity! },
    ]);
  });

  test("is not changed until the reviewer says who they are and where its observation goes", async () => {
    open(server, "#/review/1");
    await heading("Review item 1");
    await screen.findByText("Identities they matched");
    await idle();

    make("Place it", { "Where crm:c-117 goes": "new" });
    expect(await refused()).toBe("Say who you are: the change is recorded under your name.");
    make("Place it", { ...ada, "Where crm:c-117 goes": "" });
    await waitFor(async () => expect(await refused()).toBe("Choose where at least one of them goes."));

    expect(changes()).toEqual([]);
    expect(await head()).toBe(7);
  });

  test("is dismissed under the reviewer's name", async () => {
    open(server, "#/review/1", undefined, memory());
    await heading("Review item 1");
    await screen.findByText("Identities they matched");
    await idle();

    make("Dismiss", { ...ada, Reason: "not a company of ours" });
    expect(await said()).toBe("The item was dismissed. Later runs leave its observations alone.");
    await waitFor(() => expect(field("Status")).toBe("dismissed"));
    expect(field("Dismissed by")).toMatch(/^user:ada at \S+ \S+ “not a company of ours”$/);
    expect(screen.queryByRole("button", { name: "Dismiss" })).toBeNull();

    const item = await server.client.reviewItem(1);
    expect(item.status).toBe("dismissed");
    expect(item.dismissed?.by).toBe("user:ada");
    expect(changes()).toEqual(["POST /v1/review/1/dismiss"]);
  });

  test("that someone else settled first says so", async () => {
    open(server, "#/review/1");
    await heading("Review item 1");
    await screen.findByText("Identities they matched");
    await idle();
    await server.client.dismissReview(1, { actor: REVIEWER });

    make("Dismiss", ada);
    expect(await refused()).toMatch(/^review item 1: /);
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("an observation", () => {
  test("is moved to another identity, to one of its own, and out of any", async () => {
    const moved = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    const names = memory();
    names.setItem(REVIEWER_KEY, "grace");
    open(server, `#/observations/${moved.id}`, undefined, names);
    await heading("Observation");
    await screen.findByText("Move it");
    await idle();
    // The name the reviewer last used is filled in.
    expect((formOf("Move").getByLabelText("Your name") as HTMLInputElement).value).toBe("grace");

    // The identity it goes to can be named by an observation it holds.
    make("Move", { "That identity": "crm:nothing" });
    expect(await refused()).toBe("No identity is named crm:nothing.");
    make("Move", { "That identity": "crm:c-109", Reason: "one company after all" });
    expect(await said()).toBe("Moved at log position 8.");
    await waitFor(() =>
      expect(document.querySelector("dd a[href^='#/identities/']")?.getAttribute("title")).toBe(other.identity!),
    );
    expect((await entry(8)).changeset).toMatchObject({
      actor: "user:grace",
      reason: "one company after all",
      ops: [{ op: "assign", observation: moved.id, from: moved.identity!, to: other.identity! }],
    });

    make("Move", { "Where to": "new" });
    await waitFor(async () => expect(await said()).toBe("Moved at log position 9."));
    const alone = (await server.client.observation(moved.id)).identity!;
    expect([moved.identity, other.identity]).not.toContain(alone);
    await waitFor(() =>
      expect(document.querySelector("dd a[href^='#/identities/']")?.getAttribute("title")).toBe(alone),
    );

    make("Move", { "Where to": "none" });
    await waitFor(async () => expect(await said()).toBe("Taken out of its identity at log position 10."));
    await waitFor(() => expect(field("Identity")).toBe("none"));
    expect((await server.client.observation(moved.id)).identity ?? null).toBeNull();
    // It has no identity to be taken out of now, so it can only be placed.
    await screen.findByRole("button", { name: "Place" });
    const where = formOf("Place").getByLabelText("Where to") as HTMLSelectElement;
    expect([...where.options].map((option) => option.value)).toEqual(["identity", "new"]);
  });

  test("that was moved by someone else since it was read is left where it is", async () => {
    const moved = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    open(server, `#/observations/${moved.id}`);
    await heading("Observation");
    await screen.findByText("Move it");
    await idle();
    await server.client.commit({
      actor: REVIEWER,
      ops: [{ op: "assign", observation: moved.id, from: moved.identity!, to: other.identity! }],
    });

    make("Move", { ...ada, "Where to": "none" });
    const answer = await refused();
    expect(answer).toContain(`stale precondition: observation ${moved.id} was expected to belong to ${moved.identity}`);
    expect(answer).toContain("Nothing was changed.");
    expect((await server.client.observation(moved.id)).identity).toBe(other.identity!);
    expect(await head()).toBe(8);
  });
});

describe("an identity", () => {
  test("is split in two by ticking the members that are something else", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const other = await row(server.client, "crm", "c-122");
    open(server, `#/identities/${orchard.identity}`);
    await heading("Identity");
    await screen.findByText("Members (3)");
    await idle();
    expect(screen.queryByRole("button", { name: /^Split the/ })).toBeNull();

    // The reviewer said crm:c-111 and registry:X0000011 are one company, so
    // the engine does not part them.
    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${orchard.id}` }));
    make("Split the 1 ticked from the rest", ada);
    const answer = await refused();
    expect(answer).toContain("have a must_link constraint, and would belong to identities");
    expect(answer).toContain("Nothing was changed.");
    expect(await head()).toBe(7);

    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${orchard.id}` }));
    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${other.id}` }));
    make("Split the 1 ticked from the rest", { Reason: "Orchard Garden is another company" });
    expect(await said()).toMatch(/^Split at log position 8: 1 observation moved to the new identity 0\S+\.$/);
    await screen.findByText("Members (2)");

    const parted = (await server.client.observation(other.id)).identity!;
    expect(parted).not.toBe(orchard.identity!);
    expect(document.querySelector("[role=status] a[href^='#/identities/']")?.getAttribute("href")).toBe(
      `#/identities/${parted}`,
    );
    expect((await server.client.identity(orchard.identity!)).members.map((member) => member.key).sort()).toEqual([
      "X0000011",
      "c-111",
    ]);
    // The two sides are kept apart unless the reviewer says otherwise.
    expect(await server.client.constraints({ observation: other.id })).toMatchObject([
      { kind: "cannot_link", from: 8 },
    ]);
    expect((await entry(8)).changeset).toMatchObject({
      actor: "user:ada",
      reason: "Orchard Garden is another company",
    });
  });

  test("is split without keeping the sides apart when the reviewer unticks that", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const other = await row(server.client, "crm", "c-122");
    open(server, `#/identities/${orchard.identity}`);
    await heading("Identity");
    await screen.findByText("Members (3)");
    await idle();

    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${other.id}` }));
    fireEvent.click(formOf("Split the 1 ticked from the rest").getByLabelText("Keep the two sides apart from now on"));
    make("Split the 1 ticked from the rest", ada);
    expect(await said()).toMatch(/^Split at log position 8: /);
    await screen.findByText("Members (2)");
    expect(await server.client.constraints({ observation: other.id })).toEqual([]);
  });

  test("takes in another identity once the reviewer has seen what it holds", async () => {
    const atlas = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    open(server, `#/identities/${atlas.identity}`);
    await heading("Identity");
    await screen.findByText("Members (2)");
    await idle();
    expect(screen.queryByRole("button", { name: "Merge it into this one" })).toBeNull();

    make("Look it up", { "The other identity": "crm:nothing" });
    expect(await refused()).toBe("No identity is named crm:nothing.");
    make("Look it up", { "The other identity": atlas.identity! });
    await waitFor(async () => expect(await refused()).toBe("That is this identity."));

    make("Look it up", { "The other identity": "crm:c-109" });
    await screen.findByRole("button", { name: "Merge it into this one" });
    expect(screen.getByText(/is a company with 2 members/).textContent).toMatch(
      /^0\S+ is a company with 2 members: crm:c-109, registry:X0512345\. Merging moves them here/,
    );
    expect(changes()).toEqual([]);

    make("Merge it into this one", { ...ada, Reason: "one company, two offices" });
    expect(await said()).toBe("Merged at log position 8.");
    await screen.findByText("Members (4)");
    expect(screen.queryByRole("button", { name: "Merge it into this one" })).toBeNull();

    expect((await server.client.observation(other.id)).identity).toBe(atlas.identity!);
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
    open(server, `#/identities/${atlas.identity}`);
    await heading("Identity");
    await screen.findByText("Members (2)");
    await idle();

    // Someone gives it another member while it is read.
    await server.client.commit({
      actor: REVIEWER,
      ops: [{ op: "assign", observation: moved.id, from: moved.identity!, to: atlas.identity! }],
    });

    make("Look it up", { "The other identity": other.identity! });
    await screen.findByRole("button", { name: "Merge it into this one" });
    make("Merge it into this one", ada);
    const answer = await refused();
    expect(answer).toContain(
      `stale merge: the members of identity ${atlas.identity} changed at log position 8, after the changeset's base 7`,
    );
    expect(answer).toContain("Nothing was changed.");
    expect((await server.client.observation(other.id)).identity).toBe(other.identity!);

    // Read again, it shows what it holds now, and the merge is made of that.
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("Members (3)");
    fireEvent.click(screen.getByRole("button", { name: "Look it up" }));
    await screen.findByRole("button", { name: "Merge it into this one" });
    make("Merge it into this one", ada);
    expect(await said()).toBe("Merged at log position 9.");
    await screen.findByText("Members (5)");
  });

  test("says which side of a split stays, and it is the side that does", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const registered = await row(server.client, "registry", "X0000011");
    const other = await row(server.client, "crm", "c-122");
    open(server, `#/identities/${orchard.identity}`);
    await heading("Identity");
    await screen.findByText("Members (3)");
    await idle();

    // The larger side keeps the ID, whichever side was ticked.
    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${other.id}` }));
    screen.getByText("The ticked member moves to a new identity. The other 2 stay here, under this ID.");
    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${other.id}` }));
    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${orchard.id}` }));
    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${registered.id}` }));
    screen.getByText(
      "The 2 ticked are the larger side, so they stay here, under this ID. The other member moves to a new identity.",
    );

    make("Split the 2 ticked from the rest", ada);
    expect(await said()).toMatch(/^Split at log position 8: 1 observation moved to the new identity 0\S+\.$/);
    await screen.findByText("Members (2)");
    expect((await server.client.observation(orchard.id)).identity).toBe(orchard.identity!);
    expect((await server.client.observation(registered.id)).identity).toBe(orchard.identity!);
    expect((await server.client.observation(other.id)).identity).not.toBe(orchard.identity!);
  });

  test("is not split when its members changed after the screen was read", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    const registered = await row(server.client, "registry", "X0000011");
    const other = await row(server.client, "crm", "c-122");
    const added = await row(server.client, "registry", "X0000010");
    open(server, `#/identities/${orchard.identity}`);
    await heading("Identity");
    await screen.findByText("Members (3)");
    await idle();
    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${orchard.id}` }));
    fireEvent.click(screen.getByRole("checkbox", { name: `Choose observation ${registered.id}` }));
    screen.getByText(/^The 2 ticked are the larger side, so they stay here/);

    // Someone gives it a fourth member, and the ticked are no longer the
    // larger side: the engine would move them, and not the other one.
    await server.client.commit({
      actor: REVIEWER,
      ops: [{ op: "assign", observation: added.id, from: added.identity!, to: orchard.identity! }],
    });

    make("Split the 2 ticked from the rest", ada);
    expect(await refused()).toBe(
      `identity ${orchard.identity} cannot be split: its members changed at log position 8, after log position 7 that the split is based on`,
    );
    expect(await head()).toBe(8);
    expect((await server.client.identity(orchard.identity!)).members).toHaveLength(4);

    // Read again, it says what a split of what it holds now would do.
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("Members (4)");
    screen.getByText("The 2 ticked move to a new identity. The other 2 stay here, under this ID.");
    make("Split the 2 ticked from the rest", ada);
    expect(await said()).toMatch(/^Split at log position 9: 2 observations moved to the new identity 0\S+\.$/);
    expect((await server.client.observation(other.id)).identity).toBe(orchard.identity!);
    expect((await server.client.observation(orchard.id)).identity).not.toBe(orchard.identity!);
  });

  test("does not take in members the reviewer has not seen", async () => {
    const atlas = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    const moved = await row(server.client, "registry", "X0000010");
    open(server, `#/identities/${atlas.identity}`);
    await heading("Identity");
    await screen.findByText("Members (2)");
    await idle();
    make("Look it up", { "The other identity": "crm:c-109" });
    await screen.findByText(/is a company with 2 members/);
    await idle();

    // Someone gives the other identity a member after it was looked up.
    await server.client.commit({
      actor: REVIEWER,
      ops: [{ op: "assign", observation: moved.id, from: moved.identity!, to: other.identity! }],
    });

    // Reading this screen again moves on what a merge is written against,
    // so what was looked up before is no longer what would be merged.
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("The log moved on after that identity was looked up. Look it up again.");
    await idle();
    expect(screen.queryByRole("button", { name: "Merge it into this one" })).toBeNull();
    expect(screen.queryByText(/is a company with 2 members/)).toBeNull();
    expect(changes()).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: "Look it up" }));
    expect((await screen.findByText(/is a company with 3 members/)).textContent).toContain(
      "crm:c-109, registry:X0000010, registry:X0512345",
    );
    make("Merge it into this one", ada);
    expect(await said()).toBe("Merged at log position 9.");
    await screen.findByText("Members (5)");
    expect((await entry(9)).changeset).toMatchObject({ actor: "user:ada", base: 8 });
  });

  test("that was merged away offers no change", async () => {
    const { changeset } = await entry(6);
    const merge = changeset.ops.find((op) => op.op === "merge");
    if (merge?.op !== "merge") throw new Error("log position 6 has no merge");
    open(server, `#/identities/${merge.from}`);
    await heading("Identity");
    await screen.findByText("Members (3)");
    await idle();
    expect(screen.queryByText("Split it")).toBeNull();
    expect(screen.queryByText("Merge another identity into it")).toBeNull();
  });
});

describe("two observations", () => {
  test("are kept apart by a constraint, which can be removed again", async () => {
    const a = await row(server.client, "crm", "c-108");
    const b = await row(server.client, "crm", "c-109");
    open(server, `#/explain?a=${a.id}&b=${b.id}`);
    await heading("Explain");
    await screen.findByText("Different identities");
    await idle();

    // They are apart, so a constraint that they are one would not hold.
    const kind = formOf("Record it").getByLabelText("The two") as HTMLSelectElement;
    expect([...kind.options].map((option) => option.value)).toEqual(["cannot_link"]);

    make("Record it", { ...ada, Reason: "two companies of one name" });
    expect(await said()).toBe("The cannot_link constraint was recorded at log position 8.");
    await screen.findByText("Constraints between them");
    await screen.findByRole("button", { name: "Remove the cannot_link constraint" });
    expect(screen.queryByRole("button", { name: "Record it" })).toBeNull();
    expect(await server.client.constraints({ observation: a.id })).toMatchObject([{ kind: "cannot_link", from: 8 }]);
    expect((await entry(8)).changeset).toMatchObject({ actor: "user:ada", reason: "two companies of one name" });

    make("Remove the cannot_link constraint");
    await waitFor(async () => expect(await said()).toBe("The cannot_link constraint was removed at log position 9."));
    await screen.findByRole("button", { name: "Record it" });
    expect(await server.client.constraints({ observation: a.id })).toEqual([]);
  });

  test("of one identity can only be said to be one", async () => {
    const versions = (await server.client.observations({ source: "crm", key: "c-104" })).observations;
    open(server, `#/explain?a=${versions[0]!.id}&b=${versions[1]!.id}`);
    await heading("Explain");
    await screen.findByText("The same identity");
    await idle();
    const kind = formOf("Record it").getByLabelText("The two") as HTMLSelectElement;
    expect([...kind.options].map((option) => option.value)).toEqual(["must_link"]);
  });

  test("without an identity can be said to be one or to be two", async () => {
    const a = await row(server.client, "crm", "c-117");
    const b = await row(server.client, "registry", "X0000009");
    open(server, `#/explain?a=${a.id}&b=${b.id}`);
    await heading("Explain");
    await screen.findByText("Not both placed");
    await idle();

    make("Record it", { ...ada, "The two": "must_link" });
    expect(await said()).toBe("The must_link constraint was recorded at log position 8.");
    await screen.findByRole("button", { name: "Remove the must_link constraint" });
    expect(await server.client.constraints({ observation: a.id })).toMatchObject([{ kind: "must_link", from: 8 }]);
  });
});

describe("a changeset in the log", () => {
  test("is undone after the screen showed what undoing it would do", async () => {
    const orchard = await row(server.client, "crm", "c-111");
    open(server, "#/log/6");
    await heading("Log position 6");
    await screen.findByText("Operations (2)");
    await idle();

    make("Show what undoing it would do", ada);
    await screen.findByText("Undoing it would make this changeset, which holds as the store is now:");
    await waitFor(() =>
      expect(texts(".looked .ops li")).toEqual([
        expect.stringMatching(/^remove the must_link constraint on crm:c-111 and registry:X0000011$/),
        expect.stringMatching(/^reinstate identity 0/),
        expect.stringMatching(/^move observation crm:c-111 from 0\S+ to 0/),
      ]),
    );
    // Looking made a call, and changed nothing.
    expect(changes()).toEqual(["POST /v1/changesets/6/undo"]);
    expect(await head()).toBe(7);
    expect(screen.queryByRole("status")).toBeNull();

    make("Undo it", { Reason: "Orchard is not Orchard Foods" });
    expect(await said()).toBe("Undone at log position 8.");
    expect(await head()).toBe(8);
    expect((await entry(8)).changeset).toMatchObject({ actor: "user:ada", reason: "Orchard is not Orchard Foods" });
    expect((await server.client.observation(orchard.id)).identity).not.toBe(orchard.identity!);
    expect(await server.client.constraints({ observation: orchard.id })).toEqual([]);
  });

  test("that was undone before is not undone again", async () => {
    open(server, "#/log/2");
    await heading("Log position 2");
    await screen.findByText("Operations (1)");
    await idle();

    make("Undo it", ada);
    const answer = await refused();
    expect(answer).toMatch(/^changeset rejected: operation 0 \(reinstate\): identity \S+ is active/);
    expect(answer).toContain("Nothing was changed.");
    expect(texts("[role=alert] li")).toEqual([expect.stringMatching(/^Operation 0: reinstate identity 0/)]);
    expect(await head()).toBe(7);
  });
});
