// Runs `spool dev` on a copy of one of the repository's scenarios and fills
// it, so that there is something to look at while working on the studio.
//
//   bun scripts/demo.ts             the company records, with a reviewer's corrections
//   bun scripts/demo.ts news        the news items, with the mentions an extractor found
//   bun scripts/demo.ts --port 7766
//   bun scripts/demo.ts --sign-in   `spool serve`, where people sign in: it prints the link that makes the first account
//
// The server is the one `bun run dev` passes its calls on to: it listens on
// 127.0.0.1:7765. It also serves the studio it was built with, at that
// address.

import { rmSync } from "node:fs";
import { join } from "node:path";

import { binary, spool } from "../../client/test/server.ts";
import { SpoolClient } from "../src/api.ts";
import { companies, copyProject, news, seedCompanies, seedNews } from "../test/seed.ts";

const args = process.argv.slice(2);
const option = (name: string, otherwise: string) => (args.includes(name) && args[args.indexOf(name) + 1]) || otherwise;
const port = option("--port", "7765");
const signIn = args.includes("--sign-in");
const scenario = args.includes("news") ? "news" : "companies";

const project = copyProject(scenario === "news" ? news : companies);
const listen = ["--listen", `127.0.0.1:${port}`, "--store", join(project, "dev.duckdb")];
if (signIn) spool("build", project);
const child = Bun.spawn(
  signIn
    ? [
        binary(),
        "serve",
        "--manifest",
        join(project, ".spool/manifest.json"),
        "--project",
        project,
        "--tasks",
        join(project, ".spool/tasks"),
        ...listen,
      ]
    : [binary(), "dev", project, ...listen],
  { stdout: "pipe", stderr: "inherit" },
);

const stop = async (code = 0) => {
  child.kill("SIGTERM");
  await child.exited;
  rmSync(project, { recursive: true, force: true });
  process.exit(code);
};
process.on("SIGINT", () => stop());
process.on("SIGTERM", () => stop());

// The server says where it listens once it does. Nothing is sent before
// that, and nothing to an address it did not print: when another server
// has the port, this one does not start, and that one is not ours to fill.
const reader = child.stdout.getReader();
const decoder = new TextDecoder();
let printed = "";
let url: string | undefined;
while (url === undefined) {
  const { value, done } = await reader.read();
  if (done) {
    console.error("the server did not start, so nothing was filled");
    await stop(1);
    break;
  }
  process.stdout.write(value);
  printed += decoder.decode(value, { stream: true });
  url = /listening on (http:\/\/\S+)/.exec(printed)?.[1];
}
// What it prints from here on is passed on as it comes.
void (async () => {
  for (;;) {
    const { value, done } = await reader.read();
    if (done) return;
    process.stdout.write(value);
  }
})();

// On a development server the corrections are those of its `dev` account.
// Where people sign in, what fills the server is not a person: it has a
// token, made on the server's machine.
const token = signIn ? spool("token", "add", "demo", "--store", join(project, "dev.duckdb")).trim() : undefined;
const client = new SpoolClient({ url: url!, token });
await (scenario === "news" ? seedNews(client) : seedCompanies(client));
console.log(`\nthe ${scenario} scenario is at ${url}`);
await child.exited;
