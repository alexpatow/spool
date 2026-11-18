/**
 * Runs the samples in the pages, so that what a page shows is what happens.
 *
 *   bun run check:samples                 check every page
 *   bun run check:samples --update        rewrite the output blocks from what was printed
 *   bun run check:samples --spool <path>  the binary to run (default: ../target/release/spool,
 *                                         or SPOOL_BIN)
 *   bun run check:samples --port <n>      the port `spool dev` listens on, when 7765 is taken
 *   bun run check:samples --keep          leave the session's directory behind, to look at
 *   bun run check:samples --stop-at <page>:<line>
 *                                         stop before the block that opens on that line, which
 *                                         with --keep leaves the project as a reader has it there
 *
 * The pages are read in sidebar order and run as one session in one empty
 * directory, as a reader who follows them would. In a page:
 *
 *   ```yaml title="sources/crm.yaml"      a file: written, relative to the directory the
 *                                         session is in
 *   ```sh                                 commands: run, and must succeed
 *   ```text title="Output"                what the commands before it printed: compared
 *
 * An MDX comment on the line before a block, written `sample: <what>` between
 * the comment marks, changes what is done with it:
 *
 *   sample: skip <why>                    not run or written, and the reason is reported
 *   sample: background                    started and left running (`spool dev`); its output
 *                                         block is compared with what it printed first
 *   sample: existing                      a file a command made: compared, not written
 *
 * In commands, `<identity of crm:c-1>` stands for the identity of that record
 * and `<open draft>` for the one draft that is open. Both are minted, so they
 * differ on every machine. The page tells the reader to look them up; this
 * script does the same.
 *
 * Output is compared line by line. Times are compared by shape. IDs and
 * hashes must agree with themselves, within a block and from page to page:
 * where the pages show one ID twice, the run must print one ID twice, and
 * where they show two, it must print two. A line that is only `...` stands
 * for any lines. `compare-output.ts` does the comparing.
 */
import { type Subprocess, spawn, spawnSync } from "bun";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { asShown, noIds, sameOutput, tidy } from "./compare-output";

const DEFAULT_PORT = 7765;
const siteDir = resolve(import.meta.dir, "..");
const docsDir = join(siteDir, "content/docs");

const args = process.argv.slice(2);
const flag = (name: string) => args.includes(name);
const option = (name: string) => {
  const index = args.indexOf(name);
  return index === -1 ? undefined : args[index + 1];
};

const update = flag("--update");
const spoolBin = resolve(
  option("--spool") ?? process.env["SPOOL_BIN"] ?? join(siteDir, "../target/release/spool"),
);

interface Block {
  lang: string;
  title?: string;
  directive?: string;
  body: string;
  /** Lines of the page the body spans, for `--update`: [first, last) */
  start: number;
  end: number;
  line: number;
}

/** The pages with their blocks, in the order `meta.json` gives them. */
function pagesInOrder(dir: string = docsDir): string[] {
  const meta = JSON.parse(readFileSync(join(dir, "meta.json"), "utf8")) as { pages: string[] };
  return meta.pages.flatMap((entry) => {
    if (entry.startsWith("---")) return [];
    if (entry.startsWith("...")) return pagesInOrder(join(dir, entry.slice(3)));
    return [join(dir, `${entry}.mdx`)];
  });
}

function blocksOf(lines: string[]): Block[] {
  const blocks: Block[] = [];
  for (let index = 0; index < lines.length; index++) {
    const open = /^```(\w+)(.*)$/.exec(lines[index] ?? "");
    if (!open) continue;
    const start = index + 1;
    let end = start;
    while (end < lines.length && !/^```\s*$/.test(lines[end] ?? "")) end++;

    let above = index - 1;
    while (above >= 0 && (lines[above] ?? "").trim() === "") above--;
    const directive = /^\{\/\*\s*sample:\s*(.*?)\s*\*\/\}$/.exec(lines[above] ?? "")?.[1];
    const title = /title="([^"]*)"/.exec(open[2] ?? "")?.[1];

    blocks.push({
      lang: open[1] ?? "",
      body: lines.slice(start, end).join("\n"),
      start,
      end,
      line: index + 1,
      ...(title !== undefined && { title }),
      ...(directive !== undefined && { directive }),
    });
    index = end;
  }
  return blocks;
}

const failures: string[] = [];
const skipped: string[] = [];
let ran = 0;
let compared = 0;
let written = 0;

function fail(where: string, message: string) {
  failures.push(`${where}\n${message.replace(/^/gm, "    ")}`);
}

// What the pages show for each ID and hash, and what this run printed for it.
const ids = noIds();

async function freePort(port: number): Promise<boolean> {
  return new Promise((done) => {
    const server = createServer();
    server.once("error", () => done(false));
    server.listen(port, "127.0.0.1", () => server.close(() => done(true)));
  });
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
  const address = (text: string) => text.replaceAll(`127.0.0.1:${DEFAULT_PORT}`, `127.0.0.1:${port}`);
  const unaddress = (text: string) =>
    text.replaceAll(`127.0.0.1:${port}`, `127.0.0.1:${DEFAULT_PORT}`);

  const work = mkdtempSync(join(tmpdir(), "spool-samples-"));
  const env = {
    ...process.env,
    PATH: `${dirname(spoolBin)}:${process.env["PATH"] ?? ""}`,
    SPOOL_URL: `http://127.0.0.1:${port}`,
    // What a reader's terminal has not got.
    SPOOL_TOKEN: "",
    SPOOL_STORE: "",
  };
  let cwd = work;
  const background: { process: Subprocess; log: () => string }[] = [];
  const devLog = () => background.map((entry) => entry.log()).join("");

  /** Waits for `spool dev` to say what it made of a saved file. */
  async function settle(before: number) {
    if (background.length === 0) return;
    const deadline = Date.now() + 5000;
    while (Date.now() < deadline) {
      if (devLog().length > before) {
        await Bun.sleep(150);
        return;
      }
      await Bun.sleep(50);
    }
  }

  function identityOf(record: string, where: string): string {
    const result = spawnSync([spoolBin, "show", "records", record, "--json"], { cwd, env });
    try {
      const answer = JSON.parse(result.stdout.toString()) as { record: { identity: string | null } };
      if (answer.record.identity) return answer.record.identity;
    } catch {
      // Reported below.
    }
    fail(where, `no identity found for \`${record}\`: ${result.stderr.toString().trim()}`);
    return "00000000-0000-0000-0000-000000000000";
  }

  function recordOf(record: string, where: string): string {
    const result = spawnSync([spoolBin, "show", "records", record, "--json"], { cwd, env });
    try {
      return (JSON.parse(result.stdout.toString()) as { record: { id: string } }).record.id;
    } catch {
      fail(where, `no record found for \`${record}\`: ${result.stderr.toString().trim()}`);
      return "unknown";
    }
  }

  function openDraft(where: string): string {
    const result = spawnSync([spoolBin, "show", "drafts", "--status", "open", "--json"], { cwd, env });
    let ids: string[] = [];
    try {
      const answer = JSON.parse(result.stdout.toString()) as { drafts: { id: string }[] };
      ids = answer.drafts.map((draft) => draft.id);
    } catch {
      // Reported below.
    }
    if (ids.length !== 1) fail(where, `expected one open draft, found ${ids.length}`);
    return ids[0] ?? "00000000-0000-0000-0000-000000000000";
  }

  try {
    const stopAt = option("--stop-at");
    let stopped = false;
    for (const file of pagesInOrder()) {
      if (stopped) break;
      const name = file.slice(docsDir.length + 1);
      const lines = readFileSync(file, "utf8").split("\n");
      const blocks = blocksOf(lines);
      const edits: { block: Block; text: string }[] = [];
      let last: { output: string; background: boolean } | undefined;

      for (const block of blocks) {
        const where = `${name}:${block.line}`;
        const directive = block.directive ?? "";
        if (where === stopAt) {
          stopped = true;
          break;
        }

        if (directive.startsWith("skip")) {
          skipped.push(`${where}  ${directive.replace(/^skip:?\s*/, "") || "no reason given"}`);
          last = undefined;
          continue;
        }

        if (block.lang === "sh") {
          let script = address(block.body)
            .replace(/<identity of ([^>\s]+)>/g, (_match, record) => identityOf(String(record), where))
            .replace(/<record of ([^>\s]+)>/g, (_match, record) => recordOf(String(record), where))
            .replace(/<open draft>/g, () => openDraft(where));
          if (directive === "background") {
            if (port !== DEFAULT_PORT) script += ` --listen 127.0.0.1:${port}`;
            const child = spawn(["sh", "-c", `exec ${script} 2>&1`], { cwd, env, stdout: "pipe" });
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
            background.push({ process: child, log: () => log });
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

          const status = devLog().trimEnd().split("\n").pop() ?? "";
          if (status.includes("still running the last version")) {
            fail(where, `the project did not compile before this command:\n${devLog().slice(-600)}`);
          }
          const result = spawnSync(
            ["sh", "-e", "-c", `${script}\nprintf '\\n__CWD__%s' "$PWD"`],
            { cwd, env },
          );
          const [stdout = "", next] = result.stdout.toString().split("\n__CWD__");
          const stderr = result.stderr.toString();
          ran++;
          if (result.exitCode !== 0 || next === undefined) {
            fail(where, `exit ${result.exitCode}\n${block.body}\n${stdout}${stderr}`);
          } else {
            cwd = next;
          }
          last = { output: unaddress(stdout + stderr), background: false };
          continue;
        }

        if (block.lang === "text" && block.title === "Output") {
          if (!last) {
            fail(where, "an output block with no command before it");
            continue;
          }
          const actual = tidy(last.output);
          const expected = tidy(block.body);
          compared++;
          if (update) {
            if (expected.length === 0 || !sameOutput(expected, actual, ids, last.background)) {
              edits.push({ block, text: actual.map((line) => asShown(line, ids)).join("\n") });
            }
          } else if (!sameOutput(expected, actual, ids, last.background)) {
            fail(where, `the page shows:\n${expected.join("\n")}\n\nand this was printed:\n${actual.join("\n")}`);
          }
          last = undefined;
          continue;
        }

        if (block.title && block.title !== "Output") {
          const target = join(cwd, block.title);
          if (directive === "existing") {
            const found = existsSync(target) ? readFileSync(target, "utf8") : undefined;
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
    for (const entry of background) entry.process.kill();
    await Promise.all(background.map((entry) => entry.process.exited));
    if (flag("--keep")) console.log(`kept ${work}`);
    else rmSync(work, { recursive: true, force: true });
  }

  console.log(
    `${ran} command blocks run, ${written} files written, ${compared} outputs and files compared, ${skipped.length} blocks skipped`,
  );
  for (const entry of skipped) console.log(`  skipped ${entry}`);
  if (failures.length > 0) {
    console.error(`\n${failures.length} failed:\n`);
    for (const failure of failures) console.error(`${failure}\n`);
    process.exit(1);
  }
}

await main();
