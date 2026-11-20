/**
 * Runs the samples in the pages, so that what a page shows is what happens.
 *
 *   bun run check:samples                 check every page
 *   bun run check:samples --update        rewrite the output blocks from what was printed
 *   bun run check:samples --release       also fail when a page uses something that is not built
 *   bun run check:samples --spool <path>  the binary to run (default: ../target/release/spool,
 *                                         or SPOOL_BIN)
 *   bun run check:samples --port <n>      the port `spool dev` listens on, when 7765 is taken.
 *                                         Every server a page starts is moved with it, and
 *                                         nothing then listens on 7765
 *   bun run check:samples --page <name>   only the pages whose path has <name> in it
 *   bun run check:samples --trace         say which block is being run
 *   bun run check:samples --keep          leave the last session's directory behind, to look at
 *   bun run check:samples --stop-at <page>:<line>
 *                                         stop before the block that opens on that line, which
 *                                         with --keep leaves the project as a reader has it there
 *
 * The pages are read in sidebar order. A page that starts a session is run in
 * an empty directory, as a reader who lands on it would run it. In a page:
 *
 *   ```csv title="rows.csv"               a file: written, relative to the directory the
 *                                         session is in
 *   ```sh                                 commands: run, and must succeed. A variable one
 *                                         block sets is there in the blocks after it
 *   ```text title="Output"                what the block before it printed: compared
 *   ```json title="MCP tool call"         a tool's name and arguments: sent to the server's
 *                                         /mcp, and must succeed. `$NAME` stands for a
 *                                         variable an `sh` block set
 *
 * An MDX comment on the line before a block, written `sample: <what>` between
 * the comment marks, changes what is done with it. Several are divided by `;`.
 *
 *   sample: new session                   start from an empty directory, with no server
 *                                         running and no IDs remembered
 *   sample: skip <why>                    not run or written, and the reason is reported
 *   sample: background                    started and left running (`spool dev`); its output
 *                                         block is compared with what it printed first
 *   sample: stop servers                  what `background` started is stopped first
 *   sample: existing                      a file a command made: compared, not written
 *   sample: fails                         the commands are to end with an error, which the
 *                                         output block shows
 *   sample: after                         run when the session ends, not here: another way
 *                                         to do the step before it, which a reader does once
 *   sample: not built <name>              uses something `not-built.ts` lists under that
 *                                         name. It is run; if it fails it is reported as not
 *                                         built yet, and its output block is not compared
 *
 * What is not built yet is reported in one list, apart from mismatches, and
 * does not fail the check unless `--release` is given.
 *
 * The extractor that `spool init` writes finds names with a small model, and
 * its first run fetches about 330 MB. Unless SPOOL_TEST_MODEL is set, the
 * check puts `release/by-legal-form.py` in its place in each new project: the
 * stand-in that the engine's own tests use, which finds the same three names
 * in the sample note and fetches no model. A page that shows the extractor's
 * file is still compared with the file `spool init` wrote.
 *
 * Output is compared line by line. Times are compared by shape. IDs and
 * hashes must agree with themselves, within a block and through a session:
 * where the pages show one ID twice, the run must print one ID twice, and
 * where they show two, it must print two. A line that is only `...` stands
 * for any lines. `compare-output.ts` does the comparing.
 */
import { type Subprocess, spawn, spawnSync } from "bun";
import { spawn as spawnChild } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { type Ids, asShown, noIds, sameOutput, tidy } from "./compare-output";
import { notBuilt } from "./not-built";
import { listenOn, movePorts } from "./ports";
import { shownIds } from "./shown-ids";
import { type Block, blocksOf, directivesOf, shellVariables } from "./sample-blocks";

const DEFAULT_PORT = 7765;
/** How long a block may run, in milliseconds, and one that is to end with an error. */
const LIMIT = 600_000;
const LIMIT_TO_FAIL = 120_000;
const siteDir = resolve(import.meta.dir, "..");
const docsDir = join(siteDir, "content/docs");

const args = process.argv.slice(2);
const flag = (name: string) => args.includes(name);
const option = (name: string) => {
  const index = args.indexOf(name);
  return index === -1 ? undefined : args[index + 1];
};

const update = flag("--update");
const spoolBin = resolve(option("--spool") ?? process.env["SPOOL_BIN"] ?? join(siteDir, "../target/release/spool"));

/** The pages, in the order `meta.json` gives them. */
function pagesInOrder(dir: string = docsDir): string[] {
  const meta = JSON.parse(readFileSync(join(dir, "meta.json"), "utf8")) as { pages: string[] };
  return meta.pages.flatMap((entry) => {
    if (entry.startsWith("---")) return [];
    if (entry.startsWith("...")) return pagesInOrder(join(dir, entry.slice(3)));
    return [join(dir, `${entry}.mdx`)];
  });
}

/** Whether the sample extractor runs with its model, which is fetched on its first run. */
const withModel = (process.env["SPOOL_TEST_MODEL"] ?? "") !== "";
const modelStandIn = join(siteDir, "../release/by-legal-form.py");
/** The sample extractor of each new project as `spool init` wrote it, by its path. */
const sampleExtractors = new Map<string, string>();

/** Puts the stand-in in the place of every sample extractor under `dir` that has not got it yet. */
function standInForModel(dir: string, depth = 0) {
  if (withModel || depth > 2) return;
  const file = join(realpathSync(dir), "extractors/companies.py");
  if (existsSync(file) && !sampleExtractors.has(file)) {
    const written = readFileSync(file, "utf8");
    if (written.includes("fast_gliner")) {
      sampleExtractors.set(file, written);
      writeFileSync(file, readFileSync(modelStandIn, "utf8"));
    }
  }
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory() && !entry.name.startsWith(".")) standInForModel(join(dir, entry.name), depth + 1);
  }
}

const failures: string[] = [];
const skipped: string[] = [];
/** Where each thing that is not built yet is used, and where a mark is no longer needed. */
const notYet = new Map<string, string[]>();
const builtNow = new Map<string, string[]>();
let ran = 0;
let compared = 0;
let written = 0;
let sessions = 0;

function fail(where: string, message: string) {
  failures.push(`${where}\n${message.replace(/^/gm, "    ")}`);
}

function note(list: Map<string, string[]>, name: string, where: string) {
  const places = list.get(name) ?? [];
  if (!places.includes(where)) places.push(where);
  list.set(name, places);
}

async function freePort(port: number): Promise<boolean> {
  return new Promise((done) => {
    const server = createServer();
    server.once("error", () => done(false));
    server.listen(port, "127.0.0.1", () => server.close(() => done(true)));
  });
}

interface Session {
  work: string;
  cwd: string;
  ids: Ids;
  /** Shell variables that blocks have set, as `sh` prints them. */
  variables: Map<string, string>;
  background: { process: Subprocess; log: () => string }[];
  /** Blocks marked `after`. */
  later: (() => Promise<void>)[];
}

async function main() {
  if (!existsSync(spoolBin)) {
    console.error(
      `No spool binary at ${spoolBin}.\nBuild one in the repository root with\n  cargo build --release --locked -p spool-cli\nor name one with --spool or SPOOL_BIN.`,
    );
    process.exit(2);
  }

  const port = Number(option("--port") ?? DEFAULT_PORT);
  if (!(await freePort(port))) {
    console.error(
      `Port ${port} is in use, and \`spool dev\` in the pages listens there. Stop what holds it, or pass --port <n>.`,
    );
    process.exit(2);
  }
  // The pages use 7765 for `spool dev` and 7766 for a second server.
  const address = (text: string) => listenOn(movePorts(text, DEFAULT_PORT, port), DEFAULT_PORT, port);
  const unaddress = (text: string) => movePorts(text, port, DEFAULT_PORT);

  const tools = mkdtempSync(join(tmpdir(), "spool-samples-tools-"));
  // The pages run `spool`. The binary under test may have another file name,
  // and another `spool` may be installed, so the name is a link to this one,
  // ahead of everything else on the search path.
  const bin = join(tools, "bin");
  mkdirSync(bin);
  symlinkSync(realpathSync(spoolBin), join(bin, "spool"));

  const env = {
    ...process.env,
    PATH: `${bin}:${process.env["PATH"] ?? ""}`,
    SPOOL_URL: `http://127.0.0.1:${port}`,
    // What a reader's terminal has not got.
    SPOOL_TOKEN: "",
    SPOOL_STORE: "",
  };
  const ownVariables = new Set(shellVariables(spawnSync(["sh", "-c", "set"], { env }).stdout.toString()).keys());

  const startSession = (): Session => {
    sessions++;
    const work = mkdtempSync(join(tmpdir(), "spool-samples-"));
    return { work, cwd: work, ids: noIds(), variables: new Map(), background: [], later: [] };
  };
  const stopServers = async (session: Session) => {
    // A server that is asked to stop waits for its open connections, and this
    // process may still hold one, so it is not waited for long.
    for (const entry of session.background) entry.process.kill();
    await Promise.all(
      session.background.map(async (entry) => {
        const gone = await Promise.race([entry.process.exited.then(() => true), Bun.sleep(2000).then(() => false)]);
        if (!gone) {
          entry.process.kill("SIGKILL");
          await entry.process.exited;
        }
      }),
    );
    session.background = [];
  };
  const endSession = async (session: Session, keep: boolean) => {
    for (const block of session.later) await block();
    await stopServers(session);
    if (keep) console.log(`kept ${session.work}`);
    else rmSync(session.work, { recursive: true, force: true });
  };

  let session = startSession();
  const devLog = () => session.background.map((entry) => entry.log()).join("");

  /** Waits for `spool dev` to say what it made of a saved file. */
  async function settle(before: number) {
    if (session.background.length === 0) return;
    const deadline = Date.now() + 5000;
    while (Date.now() < deadline) {
      if (devLog().length > before) {
        await Bun.sleep(150);
        return;
      }
      await Bun.sleep(50);
    }
  }

  /** The process group of the block that is running. */
  let running: number | undefined;
  const stopGroup = (group: number) => {
    try {
      process.kill(-group, "SIGKILL");
    } catch {
      // Nothing of it is left.
    }
  };
  // A check that is stopped leaves no server behind: neither the block that
  // is running, which a signal from the terminal does not reach in its own
  // process group, nor the servers of the session.
  for (const [signal, code] of [
    ["SIGINT", 130],
    ["SIGTERM", 143],
  ] as const) {
    process.on(signal, () => {
      if (running !== undefined) stopGroup(running);
      for (const entry of session.background) entry.process.kill();
      process.exit(code);
    });
  }

  /**
   * Runs commands as one script, and keeps the directory and the variables it
   * ends with. A block that has not ended after `limit` milliseconds, such as
   * a server that was meant to be refused, is stopped with everything it
   * started: it runs in a process group of its own, so that what it left
   * running does not hold a port and a store after the check has ended.
   */
  async function runScript(script: string, limit: number) {
    const set = [...session.variables].map(([name, value]) => `${name}=${value}`).join("\n");
    // From a file: a shell that is given its commands as an argument lists them
    // among its variables, where their lines would be taken for variables.
    const file = join(tools, "block.sh");
    writeFileSync(file, `${set}\n${script}\nprintf '\\n__CWD__%s\\n__SET__\\n' "$PWD"\nset\n`);
    const child = spawnChild("sh", ["-e", file], {
      cwd: session.cwd,
      env,
      detached: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    running = child.pid;
    const printed = { stdout: "", stderr: "" };
    for (const name of ["stdout", "stderr"] as const) {
      child[name].setEncoding("utf8");
      child[name].on("data", (text: string) => (printed[name] += text));
    }
    let stopped = false;
    const timer = setTimeout(() => {
      stopped = true;
      if (child.pid !== undefined) stopGroup(child.pid);
    }, limit);
    const exitCode = await new Promise<number | null>((done) => {
      child.once("error", () => done(null));
      child.once("close", (code) => done(code));
    });
    clearTimeout(timer);
    running = undefined;
    const [stdout = "", rest] = printed.stdout.split("\n__CWD__");
    const [next, set2 = ""] = rest?.split("\n__SET__\n") ?? [];
    const ok = exitCode === 0 && next !== undefined;
    if (/\bspool init\b/.test(script)) standInForModel(session.work);
    if (ok) {
      session.cwd = next;
      for (const [name, value] of shellVariables(set2)) {
        if (!ownVariables.has(name)) session.variables.set(name, value);
      }
    }
    return { ok, exitCode, stopped, stdout, stderr: printed.stderr };
  }

  /** Calls a tool of the server's MCP endpoint. */
  async function callTool(body: string): Promise<{ ok: boolean; text: string }> {
    let call: { name?: unknown; arguments?: unknown };
    try {
      const filled = body.replace(/\$([A-Z][A-Z0-9_]*)/g, (whole, name) => {
        const value = session.variables.get(String(name));
        return value === undefined ? whole : value.replace(/^'(.*)'$/, "$1");
      });
      call = JSON.parse(filled) as typeof call;
    } catch (error) {
      return { ok: false, text: `the block is not JSON: ${String(error)}` };
    }
    if (typeof call.name !== "string") return { ok: false, text: "the block names no tool" };
    try {
      const response = await fetch(`http://127.0.0.1:${port}/mcp`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          accept: "application/json, text/event-stream",
          connection: "close",
        },
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          method: "tools/call",
          params: { name: call.name, arguments: call.arguments ?? {} },
        }),
      });
      const answer = (await response.json()) as {
        error?: { message?: string };
        result?: { isError?: boolean; content?: { text?: string }[] };
      };
      if (answer.error) return { ok: false, text: answer.error.message ?? "the call failed" };
      const text = (answer.result?.content ?? []).map((part) => part.text ?? "").join("\n");
      return { ok: answer.result?.isError !== true, text };
    } catch (error) {
      return { ok: false, text: `no answer from /mcp: ${String(error)}` };
    }
  }

  /** With --update: gives the identities of the sample companies the IDs the pages show for them. */
  async function nameIdentities() {
    if (!update || session.background.length === 0) return;
    try {
      const answer = (await (
        await fetch(`http://127.0.0.1:${port}/v1/records?limit=1000`, { headers: { connection: "close" } })
      ).json()) as {
        records?: {
          source: string;
          key: string;
          identity: string | null;
          evidence: { key: string; start: number; end: number } | null;
        }[];
      };
      for (const record of answer.records ?? []) {
        // A record found in a document is named by the document and its place there.
        const key = record.evidence
          ? `${record.evidence.key}[${record.evidence.start}-${record.evidence.end}]`
          : record.key;
        const shown = shownIds[`${record.source}:${key}`];
        if (!shown || !record.identity) continue;
        if (session.ids.shown.has(shown) || session.ids.printed.has(record.identity)) continue;
        session.ids.shown.set(shown, record.identity);
        session.ids.printed.set(record.identity, shown);
      }
    } catch {
      // No server answers: the IDs are shown as printed.
    }
  }

  let kept = false;
  try {
    const stopAt = option("--stop-at");
    let stopped = false;
    const only = option("--page");
    for (const file of pagesInOrder()) {
      if (stopped) break;
      const name = file.slice(docsDir.length + 1);
      if (only !== undefined && !name.includes(only)) continue;
      const lines = readFileSync(file, "utf8").split("\n");
      const blocks = blocksOf(lines);
      const edits: { block: Block; text: string }[] = [];
      /** What the block before printed, or the name of what it needs that is not built. */
      let last: { output: string; background: boolean } | { missing: string } | undefined;

      for (const block of blocks) {
        const where = `${name}:${block.line}`;
        const directives = directivesOf(block);
        if (flag("--trace")) console.error(`${where}  ${block.lang} ${block.title ?? ""} ${block.directive ?? ""}`);
        if (where === stopAt) {
          stopped = true;
          break;
        }

        if (directives.has("new session")) {
          await endSession(session, false);
          session = startSession();
          last = undefined;
        }
        if (directives.has("stop servers")) await stopServers(session);

        const skip = directives.get("skip");
        if (skip !== undefined) {
          skipped.push(`${where}  ${skip || "no reason given"}`);
          last = undefined;
          continue;
        }

        const missing = directives.get("not built");
        if (missing !== undefined && !notBuilt[missing]) {
          fail(where, `\`not built ${missing}\` names nothing in scripts/not-built.ts`);
        }
        if (missing !== undefined && notBuilt[missing]?.run === false) {
          note(notYet, missing, where);
          last = { missing };
          continue;
        }

        if (block.lang === "sh") {
          const script = address(block.body);
          if (directives.has("background")) {
            const set = [...session.variables].map(([key, value]) => `${key}=${value}`).join("\n");
            const child = spawn(["sh", "-c", `${set}\nexec ${script} 2>&1`], {
              cwd: session.cwd,
              env,
              stdout: "pipe",
            });
            let log = "";
            void (async () => {
              const decoder = new TextDecoder();
              const reader = (child.stdout as ReadableStream<Uint8Array>).getReader();
              for (;;) {
                const { done, value } = await reader.read();
                if (done) break;
                log += decoder.decode(value, { stream: true });
              }
            })();
            session.background.push({ process: child, log: () => log });
            const deadline = Date.now() + 20000;
            while (!log.includes("listening on") && Date.now() < deadline) {
              if (child.exitCode !== null) break;
              await Bun.sleep(50);
            }
            await Bun.sleep(300);
            if (!log.includes("listening on")) fail(where, `did not start:\n${log}`);
            ran++;
            last = { output: unaddress(log), background: true };
            continue;
          }

          const run = async () => {
            const status = devLog().trimEnd().split("\n").pop() ?? "";
            if (status.includes("still running the last version") && !directives.has("fails")) {
              fail(where, `the project did not compile before this command:\n${devLog().slice(-600)}`);
            }
            const limit = directives.has("fails") ? LIMIT_TO_FAIL : LIMIT;
            const result = await runScript(script, limit);
            ran++;
            const answer = result.stdout + result.stderr;
            if (result.stopped) {
              fail(
                where,
                `did not end in ${limit / 1000} seconds, and was stopped with what it had started:\n${block.body}\n${answer}`,
              );
              return { output: unaddress(answer), background: false };
            }
            const worked = result.ok && !answer.trimStart().startsWith('{"error"');
            if (missing !== undefined) {
              note(worked ? builtNow : notYet, missing, where);
              return worked ? { output: unaddress(answer), background: false } : { missing };
            }
            if (directives.has("fails")) {
              if (result.exitCode === 0) fail(where, `was to fail, and did not:\n${block.body}\n${answer}`);
            } else if (!result.ok) {
              fail(where, `exit ${result.exitCode}\n${block.body}\n${answer}`);
            }
            return { output: unaddress(answer), background: false };
          };
          if (directives.has("after")) {
            session.later.push(async () => void (await run()));
            last = undefined;
          } else {
            last = await run();
          }
          continue;
        }

        if (block.lang === "json" && block.title === "MCP tool call") {
          const run = async () => {
            const result = await callTool(block.body);
            ran++;
            if (missing !== undefined) {
              note(result.ok ? builtNow : notYet, missing, where);
              return result.ok ? { output: result.text, background: false } : { missing };
            }
            if (!result.ok) fail(where, `the tool call failed:\n${block.body}\n${result.text}`);
            return { output: result.text, background: false };
          };
          if (directives.has("after")) {
            session.later.push(async () => void (await run()));
            last = undefined;
          } else {
            last = await run();
          }
          continue;
        }

        if (block.lang === "text" && block.title === "Output") {
          if (!last) {
            fail(where, "an output block with nothing run before it");
            continue;
          }
          if ("missing" in last) {
            note(notYet, last.missing, where);
            last = undefined;
            continue;
          }
          await nameIdentities();
          const actual = tidy(last.output);
          const expected = tidy(block.body);
          compared++;
          if (update) {
            if (expected.length === 0 || !sameOutput(expected, actual, session.ids, last.background)) {
              edits.push({ block, text: actual.map((line) => asShown(line, session.ids)).join("\n") });
            }
          } else if (!sameOutput(expected, actual, session.ids, last.background)) {
            fail(where, `the page shows:\n${expected.join("\n")}\n\nand this was printed:\n${actual.join("\n")}`);
          }
          last = undefined;
          continue;
        }

        if (block.title && block.title !== "Output") {
          const target = join(session.cwd, block.title);
          if (directives.has("existing")) {
            const found = existsSync(target)
              ? (sampleExtractors.get(realpathSync(target)) ?? readFileSync(target, "utf8"))
              : undefined;
            compared++;
            if (found === undefined) {
              fail(where, `${block.title} does not exist`);
            } else if (tidy(found).join("\n") !== tidy(block.body).join("\n")) {
              if (update) edits.push({ block, text: tidy(found).join("\n") });
              else fail(where, `${block.title} is not what the page shows:\n${found}`);
            }
          } else {
            const before = devLog().length;
            mkdirSync(dirname(target), { recursive: true });
            writeFileSync(target, `${block.body}\n`);
            written++;
            if (/^(sources|transforms|entities|extractors)\//.test(block.title)) await settle(before);
          }
          last = undefined;
        }
      }

      if (edits.length > 0) {
        for (const { block, text } of edits.reverse()) {
          lines.splice(block.start, block.end - block.start, ...text.split("\n"));
        }
        writeFileSync(file, lines.join("\n"));
        console.log(`updated ${name}: ${edits.length} block${edits.length === 1 ? "" : "s"}`);
      }
    }
  } finally {
    kept = flag("--keep");
    await endSession(session, kept);
    rmSync(tools, { recursive: true, force: true });
  }

  console.log(
    `${ran} blocks run in ${sessions} sessions, ${written} files written, ${compared} outputs and files compared, ${skipped.length} blocks skipped`,
  );
  for (const entry of skipped) console.log(`  skipped ${entry}`);

  const list = (title: string, entries: Map<string, string[]>) => {
    if (entries.size === 0) return;
    const uses = [...entries.values()].reduce((sum, places) => sum + places.length, 0);
    console.log(`\n${title}: ${entries.size} thing${entries.size === 1 ? "" : "s"}, used in ${uses} blocks\n`);
    for (const name of Object.keys(notBuilt)) {
      const places = entries.get(name);
      if (!places) continue;
      console.log(`  ${name}: ${notBuilt[name]?.what ?? ""}`);
      for (const place of places) console.log(`      ${place}`);
    }
    for (const [name, places] of entries) {
      if (!notBuilt[name]) console.log(`  ${name}: not in scripts/not-built.ts\n      ${places.join("\n      ")}`);
    }
  };
  list("Not built yet", notYet);
  list("Built now, so the `not built` mark on these blocks can go", builtNow);

  if (failures.length > 0) {
    console.error(`\n${failures.length} failed:\n`);
    for (const failure of failures) console.error(`${failure}\n`);
    process.exit(1);
  }
  if (flag("--release") && notYet.size > 0) {
    console.error("\n--release: the pages use what is not built yet.");
    process.exit(1);
  }
}

await main();
