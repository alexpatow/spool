// The script that fills a development server for the studio to show.

import { afterAll, expect, test } from "bun:test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { TOKEN } from "../../client/test/server.ts";
import { SEEDING, start, stopAll } from "./harness.tsx";
import { companies } from "./seed.ts";

const studio = join(dirname(fileURLToPath(import.meta.url)), "..");

afterAll(stopAll);

test(
  "the demo fills only the server it started, not one that already listens where it was told to",
  async () => {
    // A server someone else runs, with the token the demo is given.
    const server = await start(companies, 3);
    const demo = Bun.spawn(
      [process.execPath, "scripts/demo.ts", "--port", new URL(server.url).port, "--token", TOKEN],
      { cwd: studio, stdout: "pipe", stderr: "pipe" },
    );
    const [code, printed] = await Promise.all([demo.exited, new Response(demo.stderr).text()]);

    expect(printed).toContain("Address already in use");
    const { store } = await server.client.info();
    expect(store.entity_types.map((type) => type.records)).toEqual([0]);
    expect(store.head).toBe(0);
    expect(await server.client.runs()).toEqual([]);
    expect(code).not.toBe(0);
  },
  SEEDING,
);
