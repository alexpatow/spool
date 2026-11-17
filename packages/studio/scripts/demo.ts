// Runs `spool dev` on a copy of one of the repository's scenarios and fills
// it, so that there is something to look at while working on the studio.
//
//   bun scripts/demo.ts             the company records, with a reviewer's corrections
//   bun scripts/demo.ts news        the news items, with the mentions an extractor found
//   bun scripts/demo.ts --port 7766
//   bun scripts/demo.ts --token s3cret   a server the studio has to ask the token of
//
// The server is the one `bun run dev` passes its calls on to: it listens on
// 127.0.0.1:7765 with the token `dev`. It also serves the studio it was built
// with, at that address.

import { rmSync } from "node:fs";
import { join } from "node:path";

import { binary } from "../../client/test/server.ts";
import { SpoolClient } from "../src/api.ts";
import { companies, copyProject, news, seedCompanies, seedNews } from "../test/seed.ts";

const args = process.argv.slice(2);
const option = (name: string, otherwise: string) => (args.includes(name) && args[args.indexOf(name) + 1]) || otherwise;
const port = option("--port", "7765");
const token = option("--token", "dev");
const scenario = args.includes("news") ? "news" : "companies";

const project = copyProject(scenario === "news" ? news : companies);
const url = `http://127.0.0.1:${port}`;
const child = Bun.spawn(
  [binary(), "dev", project, "--listen", `127.0.0.1:${port}`, "--store", join(project, "dev.duckdb"), "--token", token],
  { stdout: "inherit", stderr: "inherit" },
);

const stop = async () => {
  child.kill("SIGTERM");
  await child.exited;
  rmSync(project, { recursive: true, force: true });
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);

const client = new SpoolClient({ url, token });
for (let tries = 0; !(await client.healthy()); tries++) {
  if (tries > 100 || child.exitCode !== null) {
    console.error("the server did not start");
    await stop();
  }
  await Bun.sleep(100);
}
await (scenario === "news" ? seedNews(client) : seedCompanies(client));
console.log(`\nthe ${scenario} scenario is at ${url}`);
await child.exited;
