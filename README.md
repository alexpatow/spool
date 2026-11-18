# Spool

Spool pulls records out of text and rows, decides which are the same real-world thing, and gives each thing an ID that stays the same as more arrive.

It is a server you run yourself: one process, written in Rust, with its data in one [DuckDB](https://duckdb.org) file. It works from files that you keep in Git: extractors, which find the companies, people or products a text names, and matching rules, which are SQL. You drive it from the command line, over HTTP, or from an agent over MCP, and it has a browser interface for looking at the data and correcting it.

Working out which records are the same thing is called entity resolution, or record linkage. Tools that do it give you clusters whose IDs belong to one run. Spool keeps the IDs: a new record joins the identity that exists, and a better rule produces a list of suggested merges, not new IDs.

```sh
curl -fsSL https://github.com/alexpatow/spool/releases/latest/download/install.sh | sh
spool init companies
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

That loads nine rows of a CRM export from two files, then finds the three companies a call note names, and matches after each. The last command lists the twelve records with the identity of each:

```text
source and key              name                     identity
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

The twelve records are six companies. Five records are Acorn Tools: four spellings in the rows, and "Acorn Tools Ltd" in the note. Its ID was made by the first match and did not change when the later rows and the note arrived. The two Halden Groups are in different countries, and a rule keeps them apart.

Version 0.1.0, for Linux and macOS. The first commit was on 14 November 2026. It is not for deduplicating one file once, for an answer per request, or for more than one machine: before you spend time on it, read [What it is not for](#what-it-is-not-for) and [What it does not do](#what-it-does-not-do).

## Getting started

The commands above are all of it. This section says what each needs and does.

You need Linux on x86-64 or arm64, or macOS on Apple silicon, and `curl`. There is no Windows build.

**Install.** The script downloads the release for your machine, checks it against its checksum, unpacks it under `~/.local/share/spool/0.1.0` and links `~/.local/bin/spool`. It needs no root and changes nothing else. To do it by hand, download `spool-0.1.0-<target>.tar.gz` from the [release](https://github.com/alexpatow/spool/releases/tag/v0.1.0), unpack it and put its `bin` directory on your search path. Keep `bin` and `lib` together: the binary loads the DuckDB library from `../lib`. `spool --version` prints `spool 0.1.0`.

**Create a project.** A project is a directory of files that you keep in Git. `spool init companies` says what it wrote:

```text
created   companies

  sources/crm.yaml                   the columns of a CRM export
  sources/notes.yaml                 what a record found in a note may say
  entities/company/entity.yaml       what companies are compared by
  entities/company/rules/            three rules, one line of SQL each
  transforms/name_key.sql            the lower-case name the rules compare
  extractors/companies.yaml          a pattern that finds company names in text
  samples/crm-1.csv                  six rows to load first
  samples/crm-2.csv                  three more for later
  samples/notes-1.txt                a call note that names three companies
  .gitignore                         keeps the server's data, .spool/, out of Git

  cd companies
  spool dev                          runs the server on this machine
  spool load samples/crm-1.csv       loads the first rows, from a second terminal
  spool extract samples/notes-1.txt  finds the companies in the note
```

`company` is an entity type: a kind of thing you want one ID for.

**Start the server.** `spool dev` listens on `127.0.0.1:7765` only and asks nobody to sign in. Its data is in `.spool/dev.duckdb` inside the project, and it applies changes to the project's files as you save them. Leave it running.

### Load rows and match

The first file of rows is `samples/crm-1.csv`:

```csv
id,name,country
c-1,Acorn Tools,GB
c-2,ACORN TOOLS,GB
c-3,Acorn Tools Ltd,GB
c-4,Larkfield Logistics,GB
c-5,Halden Group,GB
c-6,Halden Group,US
```

`spool load` makes each row a record. Loading does not match, and loading the same file again adds nothing. `spool match` judges the records that are new, against each other and against the ones the store holds, and places them in identities. An identity is one company, with its ID and the records that belong to it. That ID is what other systems store. It is minted when the identity is created, so yours will differ from the ones here.

The first match makes four identities of the six rows. `spool explain` says why two records share one, or why they do not:

```sh
spool explain crm:c-5 crm:c-6
```

```text
`crm:c-6` belongs to identity 01a1efc6-41cd-775d-8758-e731c61371cb and `crm:c-5` to identity 01a1efc6-41cd-775d-8758-e7329447fd9a.
Run 2 (strategy `rules`) judged them `no_match` (no_match rule `different_country` held).
...
```

A rule is a file that holds a SQL condition over two records, `l` and `r`. These are the three in the project. The first says which pairs get compared at all, the second matched "Acorn Tools Ltd" to "Acorn Tools", and the third kept the two Halden Groups apart. `name_key` is `lower(trim(name))`.

```sql
-- entities/company/rules/block/name_prefix.sql
left(l.name_key, 4) = left(r.name_key, 4)

-- entities/company/rules/match/similar_name.sql
jaro_winkler_similarity(l.name_key, r.name_key) >= 0.92

-- entities/company/rules/no_match/different_country.sql
l.country <> r.country
```

### Send more rows

`samples/crm-2.csv` has three more rows, c-7 to c-9. After the second match, "Larkfeild Logistics" and "Acorn Tool" are in the identities that existed, and those have the IDs they had. Kestrel Analytics is new and has a new one. Nothing that was placed before moved.

### Extract records from text

Rows are one way in. The other is text: an extractor reads a document and says where it names a company, and each place becomes a record. The project has a sample document, `samples/notes-1.txt`:

```text
Call with Dana Reyes on 12 November. She has moved to Acorn Tools Ltd and runs
purchasing there. They now ship through Larkfield Logistics Ltd, and she asked
whether we work with Brightwater Foods Ltd, their new supplier.
```

It also has a sample extractor, `extractors/companies.yaml`. This one is a pattern, which Spool runs itself: no model, no key and nothing to install.

```yaml
entity_type: company
attribute: name
pattern: '(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b'
```

`spool extract` stores the document and runs the extractor over it. Three records come out of the note, each with the document and the characters it was found at. They are records like the rows, so the same rules match them: "Acorn Tools Ltd" and "Larkfield Logistics Ltd" join the identities the rows made, and Brightwater Foods is new. `spool show identities` lists the six companies:

```text
identity                              name                   members  created at
01a1efc6-41cd-775d-8758-e7300637659a  Acorn Tools Ltd        5        log position 1
01a1efc6-41cd-775d-8758-e731c61371cb  Halden Group           1        log position 1
01a1efc6-41cd-775d-8758-e7329447fd9a  Halden Group           1        log position 1
01a1efc6-41cd-775d-8758-e733aa086f7d  Larkfield Logistics    3        log position 1
01a1efc6-42c0-7776-b736-83fde0cb4498  Kestrel Analytics      1        log position 2
01a1efc6-43a9-7403-b9f6-1f8ee76184ed  Brightwater Foods Ltd  1        log position 3
```

A pattern finds only what is written the way it expects. An extractor can also be a Python or TypeScript file in `extractors/`, which can call a language model, and which Spool runs with `uv` or `bun`.

### Over HTTP

Each command is a call to the HTTP API, which calls loading "ingest" and a match a "run". The four steps with the rows:

```sh
curl -s -X POST 'http://127.0.0.1:7765/v1/ingest?source=crm&entity_type=company' -H 'content-type: text/csv' --data-binary @samples/crm-1.csv
curl -s -X POST http://127.0.0.1:7765/v1/runs -d '{"entity_type": "company"}'
curl -s 'http://127.0.0.1:7765/v1/records?source=crm'
curl -s 'http://127.0.0.1:7765/v1/explain?a=crm:c-5&b=crm:c-6'
```

Each answers with one line of JSON. Extraction is two more calls, `POST /v1/documents` and `POST /v1/extractions`.

### From an agent

The server speaks the Model Context Protocol at `http://127.0.0.1:7765/mcp`, and `spool dev` asks for no token. For Claude Code, and for Codex:

```sh
claude mcp add --transport http spool http://127.0.0.1:7765/mcp
codex mcp add spool --url http://127.0.0.1:7765/mcp
```

The agent gets tools that load rows (`load_rows`), start a match (`run_match`), search records and identities, explain a pair (`explain_pair`), and merge, split and undo. Ask it, for example, to load `samples/crm-2.csv`, match it, and say why c-5 and c-6 are different companies.

### In a browser

The studio is at <http://127.0.0.1:7765/>. It shows the same records, identities and explanations, and it can merge two identities, split one and undo a change. This is its page for the identity of Acorn Tools after the note, with four rows and one record from text:

![The studio's page for one identity. It is headed Acorn Tools Ltd, shows the identity's ID, says "5 records from 2 sources", and lists the four spellings of the name with the record each came from. The note's record, notes-1.txt[54-69], is beside the row c-3.](docs/studio-identity.png)

### Stop, and your own rows

Ctrl-C in the first terminal stops the server. The data stays in `.spool/dev.duckdb`. To start from nothing, stop the server and delete `.spool`. For your own rows, edit the columns in `sources/crm.yaml` and the rules, then run `spool load your-file.csv --source crm`.

## What it is not for

These follow from the design. Where something else fits better, it is named. What is said about other tools comes from reading their code and documentation in November 2026, not from running them beside Spool: see the [survey](docs/landscape-survey.md).

- **Deduplicating one file once.** Spool exists to keep IDs the same as data arrives over time. If you have one file, will get no more, and nobody stores the IDs, use [Splink](https://github.com/moj-analytical-services/splink) or [dedupe](https://github.com/dedupeio/dedupe).
- **Matching without rules of your own.** The built-in matcher runs SQL conditions that you write. There is no trained model in it. Splink, dedupe and [Zingg](https://github.com/zinggAI/zingg) each learn one. A model can also judge pairs for Spool, through code of yours called a strategy, and Spool keeps the identities: `benchmarks/resolution/people` has one that runs Splink.
- **An answer per record, inside the request.** Rows are loaded in batches and matched by runs. Loading a row does not say whether it is a duplicate. [Senzing](https://senzing.com), which is proprietary, resolves each record as it arrives.
- **The same result whatever order rows arrive in.** Two records that were each placed alone stay apart until someone merges them, even when a later rule matches them. That is the cost of IDs that do not move.
- **Exact matching on a key you trust.** If two rows are the same thing whenever their tax numbers are equal, a join in your database is enough.
- **Data that is already in the FollowTheMoney format.** [nomenklatura](https://github.com/opensanctions/nomenklatura) keeps stable IDs for people and companies in that format.
- **Streaming.** There is no connection to Kafka or to a change feed. Something else has to collect rows and send them.
- **More than one machine, or more than one writer.** One server process owns one DuckDB file and uses the cores and memory of one machine. There is no replication and no clustering.
- **Several tenants in one server.** A store holds one project.
- **Embedding in your process.** There are no Python or Node bindings. Your code calls the server over HTTP, and no client library is published for that.
- **Golden records.** Spool says which records are one thing. It does not produce one merged record with the best name and address, and it writes nothing back to a source.

## What it does not do

- **Packages.** A release is an archive for Linux on x86-64 and arm64 and for macOS on Apple silicon, and a container image. There is no Homebrew, apt or Cargo package and no Windows build. Spool is two files, the binary and the DuckDB library in `lib` beside it, and there is no single static binary.
- **Connectors.** `spool load` reads CSV files and `spool extract` reads text files. Nothing reads a database, a warehouse or a file store.
- **Reading PDF or HTML.** A document is text. Turning a PDF or a web page into text is your work.
- **Corrections from the command line.** The command line loads, extracts, matches, takes rows back, shows and explains. Merging, splitting, undoing and deciding a review item are HTTP calls, MCP tools or buttons in the studio.
- **Erasing a row.** `spool retract` takes a row that its source no longer holds out of its identity and out of matching, and the row stays in the store. A row loaded by mistake, by a person or by an agent, cannot be removed, and neither can a person's data on request.
- **A row that changes from A to B and back to A stays at B.** The second A is content the server has seen under that key, so it is ignored.
- **Limits on an agent.** Over MCP an agent can load rows and start runs, in requests of 4 MiB at most. It cannot approve a draft, add a person or make a token.
- **Access is all or nothing.** A token has full access to the API and does not expire, though it can be revoked, so an agent that may read may also load. People sign in with a password or with Google, and with no other identity provider.
- **Notifying anyone.** Nothing is pushed when an identity changes. A consumer reads the log of changes from the position it last saw.
- **Backups and upgrades.** There is no backup command, and nothing is promised about a store written by one version opening in the next.
- **Runtimes for your own code.** A strategy or an extractor in Python needs [`uv`](https://docs.astral.sh/uv/) on the machine and one in TypeScript needs [`bun`](https://bun.sh). Spool brings neither, and nothing looks for either until such a file runs.
- **Size.** The largest set it has been measured on is 5,000,000 records: 210 seconds and 13.4 GB of memory on four cores, matching the whole set from nothing, at pair precision 0.880 and recall 0.879. [benchmarks/README.md](benchmarks/README.md) has the measurements and how they were made. A request to the API may be 256 MiB at most.

## Where next

- `spool` with no arguments lists the commands, and `spool <command> --help` describes each.
- [docs/mcp.md](docs/mcp.md): the tools an agent gets, and how to connect each client.
- [docs/sign-in.md](docs/sign-in.md): running a server that other people sign in to.
- [benchmarks/README.md](benchmarks/README.md): projects with more sources and rules than the one above, on public data, and what was measured on them. `benchmarks/extraction/company-news` has extractors in Python and TypeScript.
- [docs/landscape-survey.md](docs/landscape-survey.md): the other tools that were read, and where Spool differs.
- [docs/architecture-proposal.md](docs/architecture-proposal.md): the design. What records, judgements, identities, constraints and changesets are, and how strategies run.
- Bugs and questions: [open an issue](https://github.com/alexpatow/spool/issues).

## Licence

MIT. See [LICENSE](LICENSE).
