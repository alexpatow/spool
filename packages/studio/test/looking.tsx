// What the tests that only look at the studio share: a store of the company
// records for the tests of one file, the check that looking at it changed
// nothing, and what a screen calls an identity.

import { afterAll, afterEach, beforeAll, expect, test } from "bun:test";

import { cleanup } from "@testing-library/react";
import type { Server } from "spool-testing";

import type { SpoolClient } from "../src/api.ts";
import { SEEDING, STOPPING, calls, start, stopAll } from "./harness.tsx";
import { companies, seedCompanies } from "./seed.ts";

export let server: Server;

/**
 * The tests of a file look at one store of the company records, as a
 * reviewer left them. None of them changes it through the studio.
 */
export function aStoreOfCompanies() {
  beforeAll(async () => {
    server = await start(companies, 7);
    await seedCompanies(server.client);
    calls.length = 0;
  }, SEEDING);

  afterEach(() => {
    cleanup();
    window.history.replaceState(null, "", "/");
  });

  afterAll(stopAll, STOPPING);
}

/**
 * The test that the tests before it only looked: the studio made more than
 * `calls` calls for them, and none of them was a change.
 */
export function lookingChangedNothing(most: number) {
  test("looking at the screens changes nothing", () => {
    expect(calls.length).toBeGreaterThan(most);
    expect(calls.filter((call) => !call.startsWith("GET "))).toEqual([]);
  });
}

/** An identity where a screen read what it is called: its label, and the end of its ID. */
export const CALLED = / \(…[0-9a-f]{4}\)$/;

/** What a screen that holds only the ID of an identity shows for it: its label, then the end of its ID. */
export async function shownFor(client: SpoolClient, identity: string): Promise<string> {
  const { identities } = await client.identities({ ids: [identity] });
  return `${identities[0]!.label}${identity.slice(-4)}`;
}
