/**
 * What the pages describe that is not built yet.
 *
 * The pages are written for version 0.1.0, and some of what they show does not
 * exist. The samples check reports each use of such a thing under its name
 * here, in one list that is kept apart from real mismatches. The list is done
 * when it is empty.
 *
 * A name gets onto the list in one of two ways:
 *
 * - `not-built/shim.ts` stands in for a command, and says so. Nothing in a
 *   page marks these: when the real binary has the command, the stand-in
 *   steps aside and the page is compared with the real output.
 * - A block in a page carries `sample: not built <name>`. It is run all the
 *   same. If it works, the check says the mark can go and compares the block
 *   as any other.
 *
 * An entry with `run: false` cannot be tried by the check at all, so its
 * blocks are listed until the mark is taken out by hand.
 */
export interface NotBuilt {
  what: string;
  run?: false;
}

export const notBuilt: Record<string, NotBuilt> = {
  install: {
    what: "A release: the install script, and the archive of `bin/spool` and `lib/libduckdb` for each platform",
    run: false,
  },
  image: {
    what: "The container image `ghcr.io/alexpatow/spool:0.1.0`, which the Dockerfile of `spool build` starts from",
    run: false,
  },
  "init-next": { what: "`spool init` ends with the line that names `spool load samples/crm-1.csv`" },
  "init-extract": {
    what: "`spool init` writes `sources/notes.yaml`, `extractors/companies.yaml` and `samples/notes-1.txt`, maps `notes` in `entity.yaml`, and prints their four lines",
  },
  extract: {
    what: "`spool extract <file>` stores a file as a document and runs the extractor, and an extractor can be a pattern in `extractors/<name>.yaml` that Spool runs itself. What the command prints is the spec's: the pages show the stand-in's lines",
  },
  "documents-text": { what: "`POST /v1/documents` takes one document as a `text/plain` body" },
  "extract-only": { what: "`POST /v1/extractions` runs the project's only extractor when none is named" },
  "mcp-load-documents": { what: "The MCP tool `load_documents`" },
  "mcp-run-extraction": { what: "The MCP tool `run_extraction`" },
  load: {
    what: "`spool load <file>` reads a CSV file and loads its rows. What it prints is not decided: the pages show the stand-in's lines",
  },
  match: {
    what: "`spool match` starts a match run and prints what it did. What it prints is not decided: the pages show the stand-in's lines",
  },
  "show-names": {
    what: "`spool show records` and `spool show identities` list a name, and records come in the order of source and key",
  },
  "ingest-csv": { what: "`POST /v1/ingest` takes a CSV body" },
  "explain-by-key": {
    what: "`GET /v1/explain` and the MCP tool `explain_pair` take a record as `source:key`",
  },
  "mcp-load-rows": { what: "The MCP tool `load_rows`" },
  "mcp-run-match": { what: "The MCP tool `run_match`" },
  "mcp-get-run": { what: "The MCP tool `get_run`" },
  "agent-run": {
    what: "An agent has followed Getting started over MCP, and the page names the agent and its version",
    run: false,
  },
};
