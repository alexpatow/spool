// What a reviewer changes from the studio, against the real server: a record,
// an identity, two records, and a changeset in the log. The changes to a
// draft and to a review item are in `changes.test.tsx`.

import { describe, expect, test } from "bun:test";

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";

import {
  aStoreForEachTest,
  asked,
  cancel,
  changes,
  entry,
  head,
  lookUp,
  make,
  refused,
  said,
  server,
} from "./changing.tsx";
import { author, counted, field, heading, idle, open, rowsOf, texts } from "./harness.tsx";
import { row } from "./seed.ts";

aStoreForEachTest();

describe("a record", () => {
  test("is moved to another identity, to one of its own, and out of any", async () => {
    const moved = await row(server.client, "crm", "c-108");
    const other = await row(server.client, "crm", "c-109");
    open(server, `/records/${moved.id}`);
    await heading("Atlas Trading");
    await screen.findByRole("heading", { name: "Move" });
    await idle();

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
      actor: author(server),
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
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: other.identity! }],
    });

    await make("Move", { To: "none" });
    const answer = await refused();
    expect(answer).toContain(`stale precondition: record ${moved.id} was expected to belong to ${moved.identity}`);
    expect(answer).toContain("Nothing was changed.");
    expect((await server.client.record(moved.id)).identity).toBe(other.identity!);
    expect(await head()).toBe(8);
  });

  test("that its source took back says so, and is not offered an identity", async () => {
    const taken = await row(server.client, "crm", "c-108");
    const run = await server.client.retract({ source: "crm", entity_type: "company", keys: ["c-108"] });
    expect(run.retraction).toMatchObject({ retracted: 1, detached: 1 });

    // It has no features while it is gone, so it is called by its source and key.
    open(server, `/records/${taken.id}`);
    await heading("crm:c-108");
    await waitFor(() => expect(field("Identity")).toBe("taken back"));
    expect(document.body.textContent).toContain(
      `Taken back by its source in run ${run.run}. It has no identity, and no run examines it, until the source sends the row again.`,
    );
    await idle();
    // The engine would refuse it an identity, so none is offered.
    expect(screen.queryByRole("heading", { name: "Assign" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Move" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Assign" })).toBeNull();

    // It is not among the records that wait for an identity.
    cleanup();
    open(server, "/records?source=crm&key=c-108");
    await heading("Records");
    await waitFor(() => expect(rowsOf(screen.getByRole("table")).map((line) => line[5])).toEqual(["taken back"]));

    // The run says what it took back, and where the records left their identities.
    cleanup();
    open(server, `/runs/${run.run}`);
    await heading(`Run ${run.run}`);
    await screen.findByText("Retraction");
    expect(field("Kind")).toBe("retract");
    expect(field("Keys")).toBe("1 named, 0 unknown");
    expect(field("Taken back")).toBe("1 record, 1 of them left an identity");
    expect(field("Committed")).toBe(`log position ${run.retraction!.committed!.seq}`);

    // Sent again, the row is back, and its run says so.
    const again = await server.client.load({
      source: "crm",
      entity_type: "company",
      rows: [taken.payload as Record<string, string>],
    });
    expect(again.load?.restored).toBe(1);
    cleanup();
    open(server, `/runs/${again.run}`);
    await heading(`Run ${again.run}`);
    await waitFor(() =>
      expect(field("Came back")).toBe("1 record the source had taken back, 1 of them to the identity they left"),
    );
    expect(field("Committed")).toBe(`log position ${again.load!.committed!.seq}`);
    cleanup();
    open(server, `/records/${taken.id}`);
    await heading("Atlas Trading");
    await waitFor(() => expect(field("Identity")).toBe(`Atlas Trading${taken.identity!.slice(-4)}`));
    await screen.findByRole("heading", { name: "Move" });
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
    await make("Split");
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
      actor: author(server),
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
    await make("Split");
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

    await make("Merge", { Reason: "one company, two offices" });
    expect(await said()).toBe("Merged at log position 8.");
    await counted("Records", 4);
    expect(screen.queryByRole("button", { name: "Merge" })).toBeNull();

    expect((await server.client.record(other.id)).identity).toBe(atlas.identity!);
    expect((await entry(8)).changeset).toMatchObject({
      actor: author(server),
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
      ops: [{ op: "assign", record: moved.id, from: moved.identity!, to: atlas.identity! }],
    });

    lookUp(other.identity!);
    await screen.findByRole("button", { name: "Merge" });
    await make("Merge");
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
    await make("Merge");
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

    await make("Split");
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
      ops: [{ op: "assign", record: added.id, from: added.identity!, to: orchard.identity! }],
    });

    await make("Split");
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
    await make("Split");
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
    await make("Merge");
    expect(await said()).toBe("Merged at log position 9.");
    await counted("Records", 5);
    expect((await entry(9)).changeset).toMatchObject({ actor: author(server), base: 8 });
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

    await make("Add constraint", { Reason: "two companies of one name" });
    expect(await said()).toBe("Added cannot_link at log position 8.");
    await screen.findByText("Constraints");
    await screen.findByRole("button", { name: "Remove cannot_link" });
    expect(screen.queryByRole("button", { name: "Add constraint" })).toBeNull();
    expect(await server.client.constraints({ record: a.id })).toMatchObject([{ kind: "cannot_link", from: 8 }]);
    expect((await entry(8)).changeset).toMatchObject({ actor: author(server), reason: "two companies of one name" });

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

    await make("Add constraint", { Kind: "must_link" });
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
    const dialog = await asked("Undo");
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
    expect((await entry(8)).changeset).toMatchObject({ actor: author(server), reason: "Orchard is not Orchard Foods" });
    expect((await server.client.record(orchard.id)).identity).not.toBe(orchard.identity!);
    expect(await server.client.constraints({ record: orchard.id })).toEqual([]);
  });

  test("that was undone before is not undone again", async () => {
    open(server, "/log/2");
    await heading("Log position 2");
    await counted("Operations", 1);
    await idle();

    await make("Undo");
    const answer = await refused();
    expect(answer).toMatch(/^changeset rejected: operation 0 \(reinstate\): identity \S+ is active/);
    expect(answer).toContain("Nothing was changed.");
    expect(texts("[role=alert] li")).toEqual([expect.stringMatching(/^Operation 0: reinstate identity 0/)]);
    expect(await head()).toBe(7);
  });
});
