/**
 * Reads the code blocks of a page and what the comment above each one says to
 * do with it. Used by `check-samples.ts`.
 */

export interface Block {
  lang: string;
  title?: string;
  /** What the comment above the block says after `sample:`. */
  directive?: string;
  body: string;
  /** Lines of the page the body spans, for `--update`: [first, last) */
  start: number;
  end: number;
  line: number;
}

export function blocksOf(lines: string[]): Block[] {
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

const DIRECTIVES = [
  "new session", "stop servers", "background", "existing", "after", "not built", "skip",
  "without record column", "fails",
];

/**
 * The directives of a block, each with what follows its name: the reason of a
 * `skip`, the name after `not built`, and nothing for the others. Several are
 * divided by `;`. Throws on one it does not know, so that a misspelt directive
 * is not taken for none.
 */
export function directivesOf(block: Block): Map<string, string> {
  const found = new Map<string, string>();
  for (const part of (block.directive ?? "").split(";")) {
    const text = part.trim();
    if (text === "") continue;
    const name = DIRECTIVES.find((known) => text === known || text.startsWith(`${known} `) || text.startsWith(`${known}:`));
    if (!name) throw new Error(`line ${block.line}: \`sample: ${text}\` is not a directive`);
    found.set(name, text.slice(name.length).replace(/^:?\s*/, ""));
  }
  return found;
}

/**
 * The variables in what `set` printed, each with its value as the shell wrote
 * it, which is quoted where it has to be. Only names in capitals are taken,
 * and only values on one line.
 */
export function shellVariables(set: string): Map<string, string> {
  const found = new Map<string, string>();
  for (const line of set.split("\n")) {
    const pair = /^([A-Z][A-Z0-9_]*)=(.*)$/.exec(line);
    if (!pair) continue;
    const value = pair[2] ?? "";
    const quotes = value.match(/'/g)?.length ?? 0;
    if (quotes % 2 === 0) found.set(pair[1] ?? "", value);
  }
  return found;
}

/**
 * What was printed, with the first column of its listing of records cut off:
 * the column headed `record`, which holds an ID of 36 characters. Lines before
 * the listing, and after an empty line that ends it, are left as they are.
 */
export function withoutRecordColumn(lines: string[]): string[] {
  const header = lines.findIndex((line) => /^record {2,}\S/.test(line));
  if (header === -1) return lines;
  const width = /^record {2,}/.exec(lines[header] ?? "")?.[0].length ?? 0;
  let end = lines.findIndex((line, index) => index > header && line === "");
  if (end === -1) end = lines.length;
  return lines.map((line, index) => (index >= header && index < end ? line.slice(width) : line));
}
