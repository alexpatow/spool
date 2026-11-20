// Runs `spool dev` on a copy of one of the repository's scenarios and fills
// it, so that there is something to look at while working on the studio.
//
//   bun scripts/demo.ts             the company records, with a reviewer's corrections
//   bun scripts/demo.ts news        the news items, with the mentions an extractor found
//   bun scripts/demo.ts --port 7766
//   bun scripts/demo.ts --sign-in   `spool serve`, where people sign in: it prints the link that makes the first account
//   bun scripts/demo.ts --google    the same, with "Sign in with Google". With SPOOL_GOOGLE_CLIENT_ID and
//                                   SPOOL_GOOGLE_CLIENT_SECRET set it is Google; without, a stand-in on this
//                                   machine that asks only which email to sign in as
//
// The server is the one `bun run dev` passes its calls on to: it listens on
// 127.0.0.1:7765. It also serves the studio it was built with, at that
// address.

import { rmSync } from "node:fs";
import { join } from "node:path";

import { makeHarness } from "spool-testing";

import { SpoolClient } from "../src/api.ts";
import { companies, copyProject, news, seedCompanies, seedNews } from "../test/seed.ts";
import { standInForGoogle } from "./google-stand-in.ts";

// The debug build of this checkout, unless `SPOOL_BIN` names a binary.
const { binary, spool } = makeHarness("debug");

const args = process.argv.slice(2);
const option = (name: string, otherwise: string) => (args.includes(name) && args[args.indexOf(name) + 1]) || otherwise;
const port = option("--port", "7765");
const google = args.includes("--google");
const signIn = google || args.includes("--sign-in");
// A client of Google's in the environment is passed on as it is.
const standIn = google && !process.env.SPOOL_GOOGLE_CLIENT_ID ? await standInForGoogle() : undefined;
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
  { stdout: "pipe", stderr: "inherit", env: { ...process.env, ...standIn?.environment } },
);

const stop = async (code = 0) => {
  child.kill("SIGTERM");
  await child.exited;
  await standIn?.stop();
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
if (standIn) {
  console.log(
    `"Sign in with Google" goes to a stand-in at ${standIn.issuer}, which asks only for an email.\n` +
      "Make the first account with the setup link, add a person with an email on the People page, sign out, and sign in as them.",
  );
}
await child.exited;
