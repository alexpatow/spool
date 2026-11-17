// What the tests of the screens share: a real server to open the studio
// on, and ways to read what a screen shows.

import { rmSync } from "node:fs";

import { render, screen, waitFor, within } from "@testing-library/react";

import { TOKEN, serve } from "../../client/test/server.ts";
import type { Server } from "../../client/test/server.ts";
import { App, TOKEN_KEY } from "../src/app.tsx";
import { copyProject } from "./seed.ts";

export const SEEDING = 120_000;

type Fetch = typeof fetch;

/** A `fetch` that does something before the real one is called. */
export function wrapped(call: (...args: Parameters<Fetch>) => ReturnType<Fetch>): Fetch {
  return Object.assign(call, { preconnect: fetch.preconnect });
}

/** What a browser keeps, without a browser. */
export function memory(token?: string) {
  const kept = new Map<string, string>(token === undefined ? [] : [[TOKEN_KEY, token]]);
  return {
    getItem: (name: string) => kept.get(name) ?? null,
    setItem: (name: string, value: string) => void kept.set(name, value),
    removeItem: (name: string) => void kept.delete(name),
  };
}

/** Every call the studio made, as `GET /v1/info`. */
export const calls: string[] = [];

// How many calls wait for their answer, and when one last began or ended.
let waiting = 0;
let stirred = 0;

export const recording = wrapped((input, init) => {
  calls.push(`${init?.method ?? "GET"} ${new URL(String(input)).pathname}`);
  const stir = () => {
    stirred = Date.now();
  };
  waiting++;
  stir();
  return fetch(input, init).finally(() => {
    waiting--;
    stir();
  });
});

/** How long the studio must have made no call before it is taken to have read all it wants. */
const QUIET_MS = 40;

/**
 * Waits until the studio has read what it set out to read. A screen reads
 * in steps, and a test that goes on to ask the server itself would
 * otherwise have the screen change under it.
 */
export async function idle(): Promise<void> {
  await waitFor(() => {
    if (waiting > 0 || Date.now() - stirred < QUIET_MS) throw new Error("the studio is still reading");
  });
}

/**
 * Opens the studio at an address, with the token already entered unless
 * `storage` says otherwise. `names` is where it keeps the reviewer's name.
 */
export function open(server: Server, address: string, storage = memory(TOKEN), names = memory()) {
  window.location.hash = address;
  render(<App url={server.url} fetch={recording} storage={storage} names={names} />);
  return storage;
}

/** Follows a link of the studio, as a click in a browser would. */
export function follow(link: Element) {
  const address = link.getAttribute("href");
  if (address === null || !address.startsWith("#")) throw new Error(`${link.outerHTML} is not a link of the studio`);
  window.location.hash = address;
}

export const heading = (name: string | RegExp) => screen.findByRole("heading", { level: 1, name });

/** The list of the studio's screens, beside whichever of them is shown. */
export const screens = () => within(screen.getByRole("navigation", { name: "Screens" }));

/** Waits until a section of the screen says how many things it holds. */
export async function counted(title: string, count: number): Promise<void> {
  await waitFor(() => {
    const found = [...document.querySelectorAll("h2")].find((heading) => heading.firstChild?.textContent === title);
    if (found === undefined) throw new Error(`no section is called ${title}`);
    const said = found.querySelector("[data-part=count]")?.textContent;
    if (said !== String(count)) throw new Error(`${title} holds ${said}, not ${count}`);
  });
}

/** The rows of a table, each as the text of its cells. */
export function rowsOf(table: HTMLElement): string[][] {
  return within(table)
    .getAllByRole("row")
    .slice(1)
    .map((line) =>
      within(line)
        .getAllByRole("cell")
        .map((cell) => cell.textContent ?? ""),
    );
}

/** The value beside a label in a list of fields. */
export function field(label: string, within: ParentNode = document): string | null {
  const term = [...within.querySelectorAll("dt")].find((term) => term.textContent === label);
  return term?.nextElementSibling?.textContent ?? null;
}

export const texts = (selector: string, within: ParentNode = document) =>
  [...within.querySelectorAll(selector)].map((found) => found.textContent ?? "");

/** The texts of the parts of the screen that are marked as one kind of thing, such as `ops`. */
export const parts = (name: string, within: ParentNode = document) => texts(`[data-part=${name}]`, within);

const servers: Server[] = [];
const projects: string[] = [];

/** Serves a copy of a scenario's project on a new store. */
export async function start(scenario: string, seed: number): Promise<Server> {
  const project = copyProject(scenario);
  projects.push(project);
  const server = await serve(project, seed);
  servers.push(server);
  return server;
}

/** Stops every server that was started, and removes what it was given. */
export async function stopAll(): Promise<void> {
  for (const server of servers.splice(0)) await server.stop();
  for (const project of projects.splice(0)) rmSync(project, { recursive: true, force: true });
}
