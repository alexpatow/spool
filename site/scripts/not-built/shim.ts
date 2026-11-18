/**
 * Stands in for the parts of the `spool` command line that the pages describe
 * and that are not built yet, so that the samples check can run every page as
 * it is written.
 *
 * The samples check puts this before the real binary on the search path. For
 * each command it first asks the real binary. If the real binary has the
 * command, or prints the output in the form the pages show, this steps aside
 * and the page is compared with the real thing. If not, this does what the
 * command is meant to do through the server's API as it is today, prints what
 * the pages say it prints, and writes the name of what is missing to
 * SPOOL_NOT_BUILT_LOG. `not-built.ts` says what each name stands for.
 *
 * So a page that uses `spool load` is checked today for what it says about the
 * rows and identities, and is checked for the command's own output as soon as
 * the command exists. Delete each part of this file when its command is built.
 */
import { spawnSync } from "bun";
import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const REAL = process.env["SPOOL_REAL"] ?? "";
const LOG = process.env["SPOOL_NOT_BUILT_LOG"];
const args = process.argv.slice(2);

if (!REAL) {
  console.error("shim: SPOOL_REAL does not name the real spool binary");
  process.exit(2);
}

function real(argv: string[], cwd?: string) {
  const result = spawnSync([REAL, ...argv], { ...(cwd && { cwd }), env: process.env });
  return {
    code: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}

/** Hands the command to the real binary and ends with its exit code. */
function passOn(): never {
  const result = spawnSync([REAL, ...args], { stdin: "inherit", stdout: "inherit", stderr: "inherit" });
  process.exit(result.exitCode ?? 1);
}

function notBuilt(name: string) {
  if (LOG) appendFileSync(LOG, `${name}\n`);
}

function stop(message: string): never {
  console.error(`spool: ${message}`);
  process.exit(1);
}

const VALUED = new Set([
  "--type", "--status", "--kind", "--source", "--key", "--document", "--after", "--limit", "--url",
  "--token", "--entity", "--reason", "--keys", "--extractor",
]);

/** The arguments after the command, divided into positional ones, options with a value, and flags. */
function parse(rest: string[]) {
  const positional: string[] = [];
  const options = new Map<string, string>();
  const flags = new Set<string>();
  for (let index = 0; index < rest.length; index++) {
    const arg = rest[index] ?? "";
    if (VALUED.has(arg)) options.set(arg, rest[++index] ?? "");
    else if (arg.startsWith("--")) flags.add(arg);
    else positional.push(arg);
  }
  return { positional, options, flags };
}

function table(rows: string[][]): string {
  const widths: number[] = [];
  for (const row of rows) row.forEach((cell, index) => (widths[index] = Math.max(widths[index] ?? 0, cell.length)));
  return rows.map((row) => row.map((cell, index) => cell.padEnd(widths[index] ?? 0)).join("  ").trimEnd()).join("\n");
}

function count(n: number, one: string, many = `${one}s`) {
  return `${n} ${n === 1 ? one : many}`;
}

function server(options: Map<string, string>) {
  return options.get("--url") ?? process.env["SPOOL_URL"] ?? "http://127.0.0.1:7765";
}

async function call(url: string, method: string, body?: unknown): Promise<any> {
  let response: Response;
  try {
    response = await fetch(url, {
      method,
      ...(body !== undefined && { body: JSON.stringify(body), headers: { "content-type": "application/json" } }),
    });
  } catch {
    stop(`the server at ${new URL(url).origin} cannot be reached\nStart one with \`spool dev\`, or name another with \`--url\` or SPOOL_URL.`);
  }
  const answer = (await response.json()) as any;
  if (answer?.error) stop(String(answer.error.message));
  return answer;
}

/** A CSV file with a header row, as a list of rows. An empty cell is left out of its row. */
function readCsv(path: string): Record<string, string>[] {
  const cells: string[][] = [[]];
  let cell = "";
  let quoted = false;
  const text = readFileSync(path, "utf8").replace(/\r\n?/g, "\n");
  for (let index = 0; index < text.length; index++) {
    const char = text[index];
    if (quoted) {
      if (char === '"' && text[index + 1] === '"') (cell += '"'), index++;
      else if (char === '"') quoted = false;
      else cell += char;
    } else if (char === '"') quoted = true;
    else if (char === "," || char === "\n") {
      cells[cells.length - 1]?.push(cell);
      cell = "";
      if (char === "\n") cells.push([]);
    } else cell += char;
  }
  if (cell !== "") cells[cells.length - 1]?.push(cell);
  const [header = [], ...lines] = cells.filter((line) => line.length > 0);
  return lines.map((line) =>
    Object.fromEntries(header.map((name, index) => [name, line[index] ?? ""]).filter(([, value]) => value !== "")),
  );
}

/** The only source, or the one the file is named after: `crm-1.csv` and `crm.csv` are `crm`. */
function sourceOf(file: string, sources: string[]): string | undefined {
  if (sources.length === 1) return sources[0];
  const name = basename(file).replace(/\.[^.]*$/, "");
  return sources.find((source) => name === source || name.startsWith(`${source}-`));
}

function list(names: string[]): string {
  const quoted = names.map((name) => `\`${name}\``);
  return quoted.length < 2 ? quoted.join("") : `${quoted.slice(0, -1).join(", ")} and ${quoted[quoted.length - 1]}`;
}

/** `spool load <file>`: reads a CSV file and sends its rows to the server. */
async function load(rest: string[]) {
  if (real(["load", "--help"]).code === 0) passOn();
  notBuilt("load");
  const { positional, options } = parse(rest);
  const file = positional[0];
  if (!file) stop("name the CSV file to load: spool load <file>");
  if (!existsSync(file)) stop(`${file} does not exist`);

  const info = real(["info", "--json"]);
  if (info.code !== 0) stop(info.stderr.trim().replace(/^spool: /, ""));
  const project = JSON.parse(info.stdout) as {
    sources: { name: string }[];
    entity_types: { name: string; sources: string[] }[];
  };
  const sources = project.sources.map((source) => source.name);
  const source = options.get("--source") ?? sourceOf(file, sources);
  if (!source) {
    stop(`the project has the sources ${list(sources)}, and ${basename(file)} is named after neither: say which its rows belong to with \`--source\``);
  }
  const types = project.entity_types.filter((type) => type.sources.includes(source)).map((type) => type.name);
  const type = options.get("--type") ?? (types.length === 1 ? types[0] : undefined);
  if (!type) stop(`${types.length} entity types take rows from \`${source}\`: say which with --type`);

  const rows = readCsv(file);
  const answer = await call(`${server(options)}/v1/ingest`, "POST", { source, entity_type: type, rows });
  const ingest = answer.run.ingest as { inserted: number; duplicates: number; superseded: number };
  const lines = [
    ["source", source],
    ["type", type],
    ["read", `${count(rows.length, "row")} from ${file}`],
    [
      "loaded",
      `${ingest.inserted - ingest.superseded} new, ${ingest.duplicates} already held, ${count(ingest.superseded, "new version")}`,
    ],
  ];
  if (ingest.inserted > 0) lines.push(["next", "`spool match` places them in identities"]);
  console.log(table(lines));
}

/** `spool match`: starts a match run and prints what it did. */
async function match(rest: string[]) {
  if (real(["match", "--help"]).code === 0) passOn();
  notBuilt("match");
  const { options, flags } = parse(rest);
  let type = options.get("--type");
  if (!type) {
    const info = real(["info", "--json"]);
    if (info.code !== 0) stop(info.stderr.trim().replace(/^spool: /, ""));
    const types = (JSON.parse(info.stdout) as { entity_types: { name: string }[] }).entity_types;
    if (types.length !== 1) stop(`the project has ${types.length} entity types: say which with --type`);
    type = types[0]?.name ?? "";
  }
  const url = server(options);
  const answer = await call(`${url}/v1/runs`, "POST", {
    entity_type: type,
    ...(flags.has("--rematch") && { rematch: true }),
  });
  if (answer.run === null) {
    console.log(`nothing to match: no record of \`${type}\` is new`);
    return;
  }
  const report = real(["show", "runs", String(answer.run.run), "--url", url]);
  if (report.code !== 0) stop(report.stderr.trim().replace(/^spool: /, ""));
  const [head = "", ...rest2] = report.stdout.trimEnd().split("\n\n");
  const keep = new Map<string, (value: string) => string>([
    ["run", (value) => value],
    ["type", (value) => value],
    ["examined", (value) => value.replace(/, 0 of them left for review before$/, "")],
    ["candidates", (value) => value],
    ["judgements", (value) => value.replace(/, 0 unsure$/, "")],
    ["placed", (value) => value],
    ["left out", (value) => value],
  ]);
  const lines: string[][] = [];
  for (const line of head.split("\n")) {
    const found = /^(\S+(?: \S+)*?)\s{2,}(.*)$/.exec(line);
    const tidy = found && keep.get(found[1] ?? "");
    if (found && tidy) lines.push([found[1] ?? "", tidy(found[2] ?? "")]);
  }
  console.log([table(lines), ...rest2].join("\n\n"));
}

const NOTE = `Call with Dana Reyes on 12 November. She has moved to Acorn Tools Ltd and runs
purchasing there. They now ship through Larkfield Logistics Ltd, and she asked
whether we work with Brightwater Foods Ltd, their new supplier.
`;

const PATTERN = String.raw`(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b`;

/**
 * What a project needs for the sample extraction. `extractors/companies.yaml`
 * is the file the pages show, which this version does not read, so the same
 * pattern is also written as a TypeScript extractor for the server to run.
 */
function writeExtraction(dir: string) {
  mkdirSync(join(dir, "extractors"), { recursive: true });
  writeFileSync(
    join(dir, "sources/notes.yaml"),
    "# Call notes. The columns are what a record found in a note may say: the\n# attributes an extractor gives it.\ncolumns: [name]\n",
  );
  writeFileSync(join(dir, "extractors/companies.yaml"), `entity_type: company\nattribute: name\npattern: '${PATTERN}'\n`);
  writeFileSync(
    join(dir, "extractors/companies.ts"),
    `import { extractor } from "spool-strategy";

const COMPANY = /${PATTERN}/g;

await extractor(async (task) => {
  const documents = await task.documents();
  return documents.flatMap((document) =>
    [...document.text.matchAll(COMPANY)].map((found) =>
      document.mention("company", found.index, found.index + found[0].length, { name: found[0] }),
    ),
  );
});
`,
  );
  writeFileSync(join(dir, "samples/notes-1.txt"), NOTE);
  for (const type of readdirSync(join(dir, "entities"))) {
    const file = join(dir, "entities", type, "entity.yaml");
    if (!existsSync(file)) continue;
    const text = readFileSync(file, "utf8").trimEnd();
    writeFileSync(file, `${text}\n  notes:\n    name: name\n    name_key: { from: name, transform: name_key }\n`);
  }
}

/**
 * `spool init`: writes the sample document and extractor as well, and ends by
 * naming the commands that load the sample rows and read the sample note.
 */
function init(rest: string[]) {
  const made = real(["init", ...rest]);
  process.stderr.write(made.stderr);
  const lines = made.stdout.trimEnd().split("\n");
  if (made.code !== 0 || !lines.some((line) => line.startsWith("  spool dev"))) {
    process.stdout.write(made.stdout);
    process.exit(made.code ?? 1);
  }
  const has = (start: string) => lines.some((line) => line.startsWith(`  ${start}`));
  const after = (start: string, added: string) => {
    const at = lines.findIndex((line) => line.startsWith(`  ${start}`));
    lines.splice(at + 1, 0, added);
  };
  if (!has("spool load ")) {
    notBuilt("init-next");
    after("spool dev", "  spool load samples/crm-1.csv  loads the first rows");
  }
  if (!has("extractors/companies.yaml")) {
    notBuilt("init-extract");
    const { positional } = parse(rest);
    writeExtraction(positional[0] ?? ".");
    after("sources/", "  sources/notes.yaml  what a record found in a note may say");
    after("transforms/", "  extractors/companies.yaml  a pattern that finds company names in text");
    after("samples/crm-2.csv", "  samples/notes-1.txt  a call note that names three companies");
    after("spool load ", "  spool extract samples/notes-1.txt  finds the companies in the note");
  }
  const parts = lines.map((line) => /^(  \S+(?: \S+)*?) {2,}(\S.*)$/.exec(line));
  const width = Math.max(...parts.map((part) => part?.[1]?.length ?? 0)) + 2;
  console.log(lines.map((line, index) => (parts[index] ? `${(parts[index]?.[1] ?? "").padEnd(width)}${parts[index]?.[2]}` : line)).join("\n"));
}

/** `spool info`: lists the sample extractor as the pattern the pages show. */
function info(rest: string[]) {
  const text = real(["info", ...rest]);
  process.stderr.write(text.stderr);
  const standsIn = "extractor `companies` (typescript, extractors/companies.ts)";
  if (text.code !== 0 || !text.stdout.includes(standsIn)) {
    process.stdout.write(text.stdout);
    process.exit(text.code ?? 1);
  }
  notBuilt("extract");
  const parts = text.stdout
    .trimEnd()
    .split("\n\n")
    .map((part) => part.replace(standsIn, "extractor `companies` (pattern, extractors/companies.yaml)"))
    .map((part) =>
      part.startsWith("not used by this version:")
        ? part.split("\n").filter((line) => !line.endsWith("extractors/companies.yaml")).join("\n")
        : part,
    )
    .filter((part) => part !== "not used by this version:");
  console.log(parts.join("\n\n"));
}

interface Mention {
  id: string;
  source: string;
  key: string;
  label: string | null;
  entity_type: string;
  evidence: { start: number; end: number; key: string } | null;
}

/** A mention as the pages name it: `notes:notes-1.txt[54-69]`. */
function mentionKey(record: { key: string; evidence: Mention["evidence"] }): string {
  const evidence = record.evidence;
  return evidence && /#\w+:\d+-\d+$/.test(record.key) ? `${evidence.key}[${evidence.start}-${evidence.end}]` : record.key;
}

/** `spool extract <file>`: stores a file as a document and runs the extractor over what it has not read. */
async function extract(rest: string[]) {
  if (real(["extract", "--help"]).code === 0) passOn();
  notBuilt("extract");
  const { positional, options, flags } = parse(rest);
  const url = server(options);
  const described = real(["info", "--json"]);
  if (described.code !== 0) stop(described.stderr.trim().replace(/^spool: /, ""));
  const project = JSON.parse(described.stdout) as { sources: { name: string }[]; extractors?: { name: string }[] };
  const extractors = (project.extractors ?? []).map((extractor) => extractor.name);
  if (extractors.length === 0) {
    stop("the project has no extractors: one is a file under extractors/, and `spool init` writes a sample");
  }
  const extractor = options.get("--extractor") ?? (extractors.length === 1 ? extractors[0] : undefined);
  if (!extractor) stop(`the project has the extractors ${list(extractors)}: say which with \`--extractor\``);

  const lines: string[][] = [];
  for (const file of positional) {
    if (!existsSync(file)) stop(`${file}: No such file or directory (os error 2)`);
    const text = readFileSync(file, "utf8");
    if (text === "") stop(`${basename(file)} is empty`);
    const sources = project.sources.map((source) => source.name);
    const source = options.get("--source") ?? sourceOf(file, sources);
    if (!source) {
      stop(`the project has the sources ${list(sources)}, and ${basename(file)} is named after neither: say which its documents belong to with \`--source\``);
    }
    const key = basename(file);
    const stored = await call(`${url}/v1/documents`, "POST", { source, documents: [{ key, text }], wait_ms: 60000 });
    const ingest = stored.run.ingest as { inserted: number; duplicates: number; superseded: number };
    lines.push(
      ["source", source],
      ["read", `1 document from ${file}`],
      ["loaded", `${ingest.inserted - ingest.superseded} new, ${ingest.duplicates} already held, ${count(ingest.superseded, "new version")}`],
    );
  }
  const ran = await call(`${url}/v1/extractions`, "POST", {
    extractor,
    wait_ms: 600000,
    ...(flags.has("--again") && { again: true }),
  });
  if (ran.run.error) stop(String(ran.run.error));
  const found = ran.run.extraction as { documents: number; mentions: number; inserted: number; duplicates: number };
  lines.push(["extractor", extractor]);
  if (found.documents === 0) {
    lines.push(["found", `0 records: \`${extractor}\` has read every document. \`--again\` reads them again.`]);
    console.log(table(lines));
    return;
  }
  lines.push([
    "found",
    `${count(found.mentions, "record")} in ${count(found.documents, "document")}: ${found.inserted} new, ${found.duplicates} already held`,
  ]);
  // What this run found, in every document it read, in the order of document and place.
  const sources = new Set<string>();
  for (const source of project.sources) sources.add(source.name);
  const mentions: (Mention & { ingest_run: number })[] = [];
  for (const source of sources) {
    const answer = await call(`${url}/v1/records?source=${source}&limit=1000`, "GET");
    mentions.push(...(answer.records as (Mention & { ingest_run: number })[]));
  }
  const run = (found as { ingest_run?: number }).ingest_run;
  const here = mentions
    .filter((mention) => mention.evidence && mention.ingest_run === run)
    .sort(
      (a, b) =>
        (a.evidence?.key ?? "").localeCompare(b.evidence?.key ?? "") || (a.evidence?.start ?? 0) - (b.evidence?.start ?? 0),
    );
  const listed = table(here.map((mention) => [`  ${mention.entity_type}`, mention.label ?? "-", mentionKey(mention)]));
  const width = Math.max(...lines.map((line) => (line[0] ?? "").length)) + 2;
  const parts = [table(lines)];
  if (here.length > 0) parts.push(listed);
  if (found.inserted > 0) parts.push(`${"next".padEnd(width)}\`spool match\` places them in identities`);
  console.log(parts.join("\n\n"));
}

function columns(line: string): string[] {
  return line.trimEnd().split(/\s{2,}/);
}

/** `spool show records`, as a list: with each record's name, in the order of source and key. */
function showRecords(rest: string[]) {
  const text = real(["show", ...rest]);
  const [header = "", ...lines] = text.stdout.trimEnd().split("\n");
  if (text.code !== 0 || columns(header).includes("name") || columns(header)[0] !== "record") {
    process.stdout.write(text.stdout);
    process.stderr.write(text.stderr);
    process.exit(text.code ?? 1);
  }
  notBuilt("show-names");
  const json = JSON.parse(real(["show", ...rest, "--json"]).stdout) as {
    records: (Mention & { ingested_at: string })[];
  };
  const known = new Map(json.records.map((record) => [record.id, record]));
  const rows = lines
    .map((line) => columns(line))
    .sort((a, b) => {
      const [x, y] = [known.get(a[0] ?? ""), known.get(b[0] ?? "")];
      if (!x || !y) return 0;
      return (
        x.source.localeCompare(y.source) ||
        (x.evidence && y.evidence
          ? x.evidence.key.localeCompare(y.evidence.key) || x.evidence.start - y.evidence.start
          : 0) ||
        x.key.localeCompare(y.key) ||
        x.ingested_at.localeCompare(y.ingested_at)
      );
    })
    .map(([record = "", , key = "", identity = ""]) => {
      const found = known.get(record);
      return [record, found ? `${found.source}:${mentionKey(found)}` : key, found?.label ?? "-", identity];
    });
  console.log(table([["record", "source and key", "name", "identity"], ...rows]));
}

/** `spool show identities`, as a list: with each identity's name. */
function showIdentities(rest: string[]) {
  const text = real(["show", ...rest]);
  const [header = "", ...lines] = text.stdout.trimEnd().split("\n");
  if (text.code !== 0 || columns(header).includes("name") || columns(header)[0] !== "identity") {
    process.stdout.write(text.stdout);
    process.stderr.write(text.stderr);
    process.exit(text.code ?? 1);
  }
  notBuilt("show-names");
  const json = JSON.parse(real(["show", ...rest, "--json"]).stdout) as {
    identities: { id: string; label: string | null }[];
  };
  const names = new Map(json.identities.map((identity) => [identity.id, identity.label ?? "-"]));
  const rows = lines.map((line) => {
    const [identity = "", ...others] = columns(line);
    return [identity, names.get(identity) ?? "-", ...others];
  });
  const [, ...titles] = columns(header);
  console.log(table([["identity", "name", ...titles], ...rows]));
}

const [command, ...rest] = args;
if (command === "load") await load(rest);
else if (command === "match") await match(rest);
else if (command === "extract" && !rest.includes("--help")) await extract(rest);
else if (command === "init" && !rest.includes("--help")) init(rest);
else if (command === "info" && !rest.includes("--json") && !rest.includes("--help")) info(rest);
else if (command === "show" && !rest.includes("--json") && !rest.includes("--help")) {
  const { positional } = parse(rest);
  if (positional[0] === "records" && positional.length === 1) showRecords(rest);
  else if (positional[0] === "identities" && positional.length === 1) showIdentities(rest);
  else passOn();
} else passOn();
