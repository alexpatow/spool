# Spool

Spool pulls records out of text and rows, decides which are the same real-world thing, and gives each thing an ID that stays the same as more arrive.

It's a self-hosted server: a single Rust binary with an embedded [DuckDB](https://duckdb.org) store. Your project is a Git repo of extractors and matching rules. Extractors are Python or TypeScript that find the companies, people or products a text names. Rules are one SQL condition per file. Drive Spool from the CLI, the HTTP API or an agent over MCP, and review and fix matches in the studio, its web UI.

Matching records that describe the same thing is called entity resolution, or record linkage. Most tools for it give you clusters whose IDs only mean something within one run. Spool keeps the IDs: a new record joins the identity that already exists, and a better rule produces suggested merges, not new IDs.

```sh
curl -fsSL https://github.com/alexpatow/spool/releases/latest/download/install.sh | sh
spool init companies --example companies
cd companies
spool dev
```

In a second terminal, in `companies`:

```sh
spool load samples/crm-1.csv
spool match
spool load samples/crm-2.csv
spool match
spool extract samples/notes-1.txt
spool match
spool show records
```

That loads nine rows of a CRM export in two batches, then finds three companies in a call note, matching after each step:

```text
SOURCE AND KEY              NAME                     IDENTITY
crm:c-1                     Acorn Tools              01a1efc6-41cd-775d-8758-e7300637659a
crm:c-2                     ACORN TOOLS              01a1efc6-41cd-775d-8758-e7300637659a
crm:c-3                     Acorn Tools Ltd          01a1efc6-41cd-775d-8758-e7300637659a
crm:c-4                     Larkfield Logistics      01a1efc6-41cd-775d-8758-e733aa086f7d
crm:c-5                     Halden Group             01a1efc6-41cd-775d-8758-e7329447fd9a
crm:c-6                     Halden Group             01a1efc6-41cd-775d-8758-e731c61371cb
crm:c-7                     Larkfeild Logistics      01a1efc6-41cd-775d-8758-e733aa086f7d
crm:c-8                     Kestrel Analytics        01a1efc6-42c0-7776-b736-83fde0cb4498
crm:c-9                     Acorn Tool               01a1efc6-41cd-775d-8758-e7300637659a
notes:notes-1.txt[54-69]    Acorn Tools Ltd          01a1efc6-41cd-775d-8758-e7300637659a
notes:notes-1.txt[119-142]  Larkfield Logistics Ltd  01a1efc6-41cd-775d-8758-e733aa086f7d
notes:notes-1.txt[179-200]  Brightwater Foods Ltd    01a1efc6-43a9-7403-b9f6-1f8ee76184ed
```

Twelve records, six companies. Acorn Tools is five records: four spellings in the CRM and "Acorn Tools Ltd" in the note. Its ID was minted by the first match and survived both later batches. The two Halden Groups are in different countries, and a rule keeps them apart.

Spool 0.4.0 runs on Linux and macOS and is MIT licensed. The [docs](https://spool.alexpatow.com/docs) cover everything below in more depth.

## Getting started

### Requirements

- Linux (x86-64 or arm64, glibc 2.34+) or macOS on Apple silicon.
- `curl` and `tar`.
- Git for `spool init`.
- [`uv`](https://docs.astral.sh/uv/), for the step that runs the sample extractor.

### Install

The script downloads the build for your platform, verifies its checksum, unpacks it to `~/.local/share/spool/0.4.0` and symlinks `~/.local/bin/spool`. It also links the man pages and the bash, zsh and fish completions under `~/.local/share`. It doesn't need root or touch your shell config. If `~/.local/bin` isn't on your `PATH`, it prints the line to add.

To install by hand, unpack `spool-0.4.0-<target>.tar.gz` from the [release](https://github.com/alexpatow/spool/releases/tag/v0.4.0) and put its `bin/` on your `PATH`. Keep `bin/` and `lib/` side by side, because the binary loads DuckDB from `../lib`. `spool --version` prints `spool 0.4.0`.

`spool update` brings an installation made with the script to the latest stable release. Run it from a project to refresh that project's coding-agent skills too.

### Create a project

`spool init companies --example companies` creates `companies/` with the sample project, initializes its Git repository, and lists what it wrote:

```text
created   companies

  sources/crm.yaml                   the columns of a CRM export
  sources/notes.yaml                 what a record found in a note may say
  entities/company/entity.yaml       what companies are compared by
  entities/company/rules/            three rules, one line of SQL each
  transforms/name_key.sql            the lower-case name the rules compare
  extractors/companies.py            finds company names in text, with a small model
  samples/crm-1.csv                  six rows to load first
  samples/crm-2.csv                  three more for later
  samples/notes-1.txt                a call note that names three companies
  .gitignore                         keeps the server's data, .spool/, and .env out of Git
  .env.example                       the form of .env, where your code's secrets go
  AGENTS.md                          what a coding agent should know here; CLAUDE.md imports it
  .agents/skills/                    skills that teach coding agents Spool, linked from .claude/skills/

  cd companies
  spool dev                          runs the server on this machine
  spool load samples/crm-1.csv       loads the first rows, from a second terminal
  spool extract samples/notes-1.txt  finds the companies in the note

The extractor is a Python file, which Spool runs with `uv`. Its first run fetches
about 330 MB, most of it the model, and later runs fetch nothing.
```

`company` is an **entity type**: a kind of thing you want one ID for.

`spool dev` serves on `127.0.0.1:7765` with no sign-in, stores data in `.spool/dev.duckdb`, and reloads project files as you save them. Leave it running.

### Load rows and match

`samples/crm-1.csv`:

```csv
id,name,country
c-1,Acorn Tools,GB
c-2,ACORN TOOLS,GB
c-3,Acorn Tools Ltd,GB
c-4,Larkfield Logistics,GB
c-5,Halden Group,GB
c-6,Halden Group,US
```

`spool load` turns each row into a **record**. Loads are idempotent, and they don't match. `spool match` compares new records with each other and with everything already placed, then puts each one in an **identity**: one company, with an ID and its records. The ID is what other systems store. It's minted when the identity is created, so yours will differ from the ones shown here.

The first match makes four identities from six rows. `spool explain` says why two records share one, or don't:

```sh
spool explain crm:c-5 crm:c-6
```

```text
`crm:c-6` belongs to identity 01a1efc6-41cd-775d-8758-e731c61371cb and `crm:c-5` to identity 01a1efc6-41cd-775d-8758-e7329447fd9a.
Run 2 (strategy `rules`) judged them `no_match` (no_match rule `different_country` held).
...
```

A rule is a SQL condition over two records, `l` and `r`. The sample has three. The block rule picks which pairs get compared, the match rule linked "Acorn Tools Ltd" to "Acorn Tools", and the no_match rule kept the Halden Groups apart. `name_key` is `lower(trim(name))`.

```sql
-- entities/company/rules/block/name_prefix.sql
left(l.name_key, 4) = left(r.name_key, 4)

-- entities/company/rules/match/similar_name.sql
jaro_winkler_similarity(l.name_key, r.name_key) >= 0.92

-- entities/company/rules/no_match/different_country.sql
l.country <> r.country
```

### Send more rows

`samples/crm-2.csv` adds c-7 to c-9. After the second match, "Larkfeild Logistics" and "Acorn Tool" join their existing identities, which keep their IDs. Kestrel Analytics is new and gets a new one. Nothing placed earlier moves.

### Extract records from text

An extractor reads a document and reports where it names a company, and each place becomes a record. The sample document is `samples/notes-1.txt`:

```text
Call with Dana Reyes on 12 November. She has moved to Acorn Tools Ltd and runs
purchasing there. They now ship through Larkfield Logistics Ltd, and she asked
whether we work with Brightwater Foods Ltd, their new supplier.
```

The sample extractor, `extractors/companies.py`, asks a small local model where the text names a company. Spool runs it with `uv`. The first run downloads about 330 MB, mostly the model, and nothing leaves your machine.

`spool extract` stores the document and runs the extractor. Each of the three records keeps its document and character range, and the same rules match them as the rows. "Acorn Tools Ltd" and "Larkfield Logistics Ltd" join existing identities, and Brightwater Foods is new. `spool show identities`:

```text
IDENTITY                              NAME                   MEMBERS  NAMES  UNLINKED PAIRS  CREATED AT
01a1efc6-41cd-775d-8758-e7300637659a  Acorn Tools Ltd        5        3      0%              log position 1
01a1efc6-41cd-775d-8758-e731c61371cb  Halden Group           1        1      -               log position 1
01a1efc6-41cd-775d-8758-e7329447fd9a  Halden Group           1        1      -               log position 1
01a1efc6-41cd-775d-8758-e733aa086f7d  Larkfield Logistics    3        3      0%              log position 1
01a1efc6-42c0-7776-b736-83fde0cb4498  Kestrel Analytics      1        1      -               log position 2
01a1efc6-43a9-7403-b9f6-1f8ee76184ed  Brightwater Foods Ltd  1        1      -               log position 3
```

Change the word the model looks for, "company", and it finds other things. Extractors are Python or TypeScript files in `extractors/`, run with `uv` or `bun`, and yours can call a hosted model.

### Over HTTP

Every command is an HTTP call. The four row steps:

```sh
curl -s -X POST 'http://127.0.0.1:7765/v1/load?source=crm&entity_type=company' -H 'content-type: text/csv' --data-binary @samples/crm-1.csv
curl -s -X POST http://127.0.0.1:7765/v1/match -d '{"entity_type": "company"}'
curl -s 'http://127.0.0.1:7765/v1/records?source=crm'
curl -s 'http://127.0.0.1:7765/v1/explain?a=crm:c-5&b=crm:c-6'
```

Each returns one line of JSON. Extraction is `POST /v1/documents`, then `POST /v1/extractions`.

### From an agent

The server speaks the Model Context Protocol at `http://127.0.0.1:7765/mcp`, with no token under `spool dev`. For Claude Code and Codex:

```sh
claude mcp add --transport http spool http://127.0.0.1:7765/mcp
codex mcp add spool --url http://127.0.0.1:7765/mcp
```

The agent gets tools to load rows and documents (`load_rows`, `load_documents`), run an extractor (`run_extraction`), match (`run_match`), search records and identities, explain a pair (`explain_pair`), and merge, split and undo. Try: "Load samples/crm-2.csv, match it, and tell me why c-5 and c-6 are different companies."

### In the studio

The studio is at <http://127.0.0.1:7765/>. It shows the same records, identities and explanations, and lets you merge, split and undo. Here's Acorn Tools after the note, with four rows and one record from text:

![The studio's page for one identity. It is headed Acorn Tools Ltd, shows the identity's ID, says "5 records from 2 sources", and lists the four spellings of the name with the record each came from. The note's record, notes-1.txt[54-69], is beside the row c-3.](docs/studio-identity.png)

### Your own data

Ctrl-C stops the server. Data stays in `.spool/dev.duckdb`. Delete `.spool/` to start over.

`spool init <name>` creates an empty project with a Git repository, the standard folders, a `.gitignore` and a `.env.example`. With no name it uses the current folder, which can contain `.git` and `.DS_Store`. Copy what you need from the sample, then `spool load your-file.csv`. Put keys for your extractors and strategies in `.env`, which Git ignores. Spool passes them to your code and never prints them.

### With a coding agent

`spool init` also sets a project up for Claude Code, Codex and Cursor. It writes an `AGENTS.md` and skills in `.agents/skills/`, symlinked from `.claude/skills/`, that teach the agent to declare sources, write rules and extractors, and run the commands for your version of `spool`. `spool update-skills` refreshes them to the installed version. See [Coding agents](https://spool.alexpatow.com/docs/agents).

## When to use something else

- **One file, deduplicated once, with nothing storing the IDs:** [Splink](https://github.com/moj-analytical-services/splink) or [dedupe](https://github.com/dedupeio/dedupe).
- **A decision inside every request:** [Senzing](https://senzing.com) resolves each record as it arrives.
- **People and companies already in FollowTheMoney:** [nomenklatura](https://github.com/opensanctions/nomenklatura).

[Compared with other tools](https://spool.alexpatow.com/docs/compared) goes through each. [Limits](https://spool.alexpatow.com/docs/limits) lists what 0.4.0 doesn't do.

## Where next

- `spool` lists the commands and `spool <command> --help` explains each. Commands that print results take `-o json` or `-o ndjson`, and list commands also take `-o csv`. The [command reference](https://spool.alexpatow.com/docs/cli-reference) has every option, and `spool completions <shell>` prints a completion script.
- [docs/mcp.md](docs/mcp.md): connecting Claude Code, Claude Desktop, Codex and other MCP clients.
- [docs/sign-in.md](docs/sign-in.md): running a server other people sign in to.
- [benchmarks/README.md](benchmarks/README.md): larger projects on public data, benchmarked up to 5,000,000 records. `benchmarks/extraction/company-news` has extractors in Python and TypeScript.
- [docs/architecture-proposal.md](docs/architecture-proposal.md): the design, covering records, judgements, identities, constraints, changesets and strategies.
- [Credits](https://spool.alexpatow.com/docs/credits) lists the projects that shaped Spool's design.
- Bugs and questions: [open an issue](https://github.com/alexpatow/spool/issues).

## Check generated files before committing

After staging your changes, run `./scripts/check-generated.sh`. It checks the staged versions of the affected generated files, including the Studio bundle and shared assets, HTTP schema and client types, CLI help and command reference, and TypeScript strategy helper. It uses Bun for TypeScript and Cargo for Rust as needed.

With Git 2.54 or newer, run `./scripts/check-generated.sh --install-hook` once per clone. The hook runs alongside existing hooks in current and future linked worktrees that contain the script. To remove it, run `git config --local --remove-section hook.spool-check-generated`. With older Git, you can call `./scripts/check-generated.sh` from your `pre-commit` hook.

If a check fails, run the regeneration command it prints, stage the updated files, and check again. If you removed a CLI command, also remove its obsolete help snapshot.

## Licence

MIT. See [LICENSE](LICENSE).
