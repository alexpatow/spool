// What the tests of a reviewer's changes share: a store for each test, and
// the dialog a change is made from.

import { afterEach, beforeEach, expect } from "bun:test";

import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { Server } from "spool-testing";

import { SEEDING, STOPPING, calls, idle, start, stopAll } from "./harness.tsx";
import { companies, seedCompanies } from "./seed.ts";

export let server: Server;

/** Every test changes what the store holds, so each has a store of its own. */
export function aStoreForEachTest() {
  beforeEach(async () => {
    server = await start(companies, 7);
    await seedCompanies(server.client);
    calls.length = 0;
  }, SEEDING);

  afterEach(async () => {
    cleanup();
    window.history.replaceState(null, "", "/");
    await stopAll();
  }, STOPPING);
}

// A change is proposed by a button, and made from the dialog the button
// opens: it says what the change would do, and takes the reviewer's reason.
// It is recorded under whoever is signed in.

/**
 * Whether a dialog is open. Asked as a yes or no: an element that an
 * expectation fails on is written out whole, and a page is long.
 */
export const asking = () => screen.queryByRole("dialog") !== null;

/** The dialog a button opens. It is opened unless one already is. */
export async function asked(button: string | RegExp) {
  if (!asking()) fireEvent.click(screen.getByRole("button", { name: button }));
  return within(await screen.findByRole("dialog"));
}

/** Fills the fields of a button's dialog by their labels. */
export async function fill(button: string | RegExp, fields: Record<string, string>) {
  const dialog = await asked(button);
  for (const [label, value] of Object.entries(fields)) {
    fireEvent.change(dialog.getByLabelText(label), { target: { value } });
  }
  return dialog;
}

/** Fills a button's dialog and presses the button in it that makes the change. */
export async function make(button: string | RegExp, fields: Record<string, string> = {}) {
  const dialog = await fill(button, fields);
  fireEvent.click(dialog.getByRole("button", { name: button }));
}

/** Leaves a dialog without making its change. */
export async function cancel() {
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(asking()).toBe(false));
}

/** Looks up the identity that another is merged with, on the screen of that other. */
export function lookUp(reference?: string) {
  if (reference !== undefined) {
    fireEvent.change(screen.getByLabelText("Identity to merge into this one"), { target: { value: reference } });
  }
  fireEvent.click(screen.getByRole("button", { name: "Find" }));
}

/** What the screen says came of the last change. */
export async function said(): Promise<string | null> {
  // A change that was made closes its dialog, and the screen says what came of it.
  await waitFor(() => expect(asking()).toBe(false));
  const status = await screen.findByRole("status");
  await idle();
  return status.textContent;
}

/** Why the screen says a change was not made. */
export async function refused(): Promise<string> {
  const alert = await screen.findByRole("alert");
  await idle();
  return alert.textContent ?? "";
}

/** The calls the studio made that were not reads. */
export const changes = () => calls.filter((call) => !call.startsWith("GET "));

export const head = async () => (await server.client.info()).store.head;

export const entry = async (seq: number) => (await server.client.changesets({ after: seq - 1, limit: 1 })).changesets[0]!;
