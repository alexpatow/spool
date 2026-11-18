# Spool

Spool decides which records are the same real-world thing, and gives each thing an ID that stays the same as more records arrive.

You have rows from several places that describe the same companies, people or products, written differently in each. You want one ID per company that other systems can store. More rows arrive every day, and the ID you handed out yesterday must still mean the same company tomorrow, also after you change the matching rules. Working out which rows are the same thing is called entity resolution, or record linkage. Spool does that and keeps the IDs.

Spool is a server that you run yourself. It is written in Rust, keeps its data in one [DuckDB](https://duckdb.org) file, and is driven by a directory of YAML and SQL files that you keep in Git. It has an HTTP API, a command line that shows and explains what the store holds, and a browser interface for looking at the data and correcting it.

It is early. Nothing is published: there is no release, no package and no container image. You build it from source, and its formats and API can still change. Before you spend time on it, read [What it is not for](#what-it-is-not-for) and [What it does not do yet](#what-it-does-not-do-yet).

## Example

A project says where rows come from and what makes two of them the same thing. This one is what `spool init` writes: one source of rows, `crm`, one entity type, `company` (an entity type is a kind of thing you want one ID for), and one rule. The rule is a SQL condition over two records, `l` and `r`, in `entities/company/rules/match/same_name.sql`:

```sql
l.name_key = r.name_key
```

`name_key` is `lower(trim(name))`, so two companies match when their names are equal but for case.

Three rows go to a running server, which [Getting started](#getting-started) sets up. The first two are one company:

```sh
curl -s -X POST http://127.0.0.1:7765/v1/ingest -H 'content-type: application/json' -d '{
  "source": "crm",
  "entity_type": "company",
  "rows": [
    { "id": "c-1", "name": "Acorn Tools" },
    { "id": "c-2", "name": "ACORN TOOLS" },
    { "id": "c-3", "name": "Larkfield Logistics" }
  ]
}'
curl -s -X POST http://127.0.0.1:7765/v1/runs -H 'content-type: application/json' -d '{"entity_type": "company"}'
```

The first call loads the rows, and each row becomes a record. The second matches the records that are new. Each call answers with one line of JSON about the run it started. `spool show records` prints what the store holds afterwards:

```sh
spool show records
```

```text
record                                type     source and key  identity
13687e30-654c-c22c-4a23-73e8cf22f9c0  company  crm:c-1         01a1ef3c-2258-734b-bae0-dad35f07f267
2aa3e6d3-813d-35c4-c75c-47f0745f9bf2  company  crm:c-2         01a1ef3c-2258-734b-bae0-dad35f07f267
68614bf6-fdf1-b12a-7d7d-7401f9c2a0c8  company  crm:c-3         01a1ef3c-2258-734b-bae0-dad4e899cf2f
```

An identity is one real-world thing, here a company, with its ID and the records that belong to it. `crm:c-1` and `crm:c-2` have the same identity, and its ID, in the last column, is the company's ID.

Later two more rows arrive: another spelling of Acorn Tools, and a company that is new.

```sh
curl -s -X POST http://127.0.0.1:7765/v1/ingest -H 'content-type: application/json' -d '{
  "source": "crm",
  "entity_type": "company",
  "rows": [
    { "id": "c-4", "name": "Acorn tools" },
    { "id": "c-5", "name": "Kestrel Analytics" }
  ]
}'
curl -s -X POST http://127.0.0.1:7765/v1/runs -H 'content-type: application/json' -d '{"entity_type": "company"}'
spool show records
```

```text
record                                type     source and key  identity
118b3805-5b71-8a29-ec51-2a3ffc810f82  company  crm:c-5         01a1ef3c-22d1-77b8-a117-5a4d18016987
13687e30-654c-c22c-4a23-73e8cf22f9c0  company  crm:c-1         01a1ef3c-2258-734b-bae0-dad35f07f267
29f842c5-b478-a142-da89-59e5b57b065c  company  crm:c-4         01a1ef3c-2258-734b-bae0-dad35f07f267
2aa3e6d3-813d-35c4-c75c-47f0745f9bf2  company  crm:c-2         01a1ef3c-2258-734b-bae0-dad35f07f267
68614bf6-fdf1-b12a-7d7d-7401f9c2a0c8  company  crm:c-3         01a1ef3c-2258-734b-bae0-dad4e899cf2f
```

`crm:c-4` joined the identity that `crm:c-1` and `crm:c-2` had, and that identity kept its ID. `crm:c-5` got a new one. Nothing that was placed before moved, so whatever stored the first ID still points at the same company.

The ID of an identity is minted when the identity is created, so yours will differ from these. The ID of a record is derived from its source, its key and its content, so yours will be the same.

`spool explain` says why two records share an identity, or why they do not:

```sh
spool explain crm:c-1 crm:c-4
```

```text
`crm:c-1` and `crm:c-4` belong to the same identity, 01a1ef3c-2258-734b-bae0-dad35f07f267.
Run 5 (strategy `rules`) judged them `match` (match rule `same_name` held).
`crm:c-1` was placed at log position 1 by `engine` (reconcile run 3, "reconcile new records").
`crm:c-4` was placed at log position 2 by `engine` (reconcile run 6, "reconcile new records").
By the rules as they are now, block rule `same_name` makes them a candidate pair and match rule `same_name` holds, so they would be judged `match`.

left   13687e30-654c-c22c-4a23-73e8cf22f9c0  crm:c-1  identity 01a1ef3c-2258-734b-bae0-dad35f07f267
right  29f842c5-b478-a142-da89-59e5b57b065c  crm:c-4  identity 01a1ef3c-2258-734b-bae0-dad35f07f267

features
            left         right
  name      Acorn Tools  Acorn tools
  name_key  acorn tools  acorn tools

rules as they are now
  block  same_name  holds
  match  same_name  holds

runs that compared them
  run 5  rules  succeeded  block: same_name  match
```

The server also answers a browser, with an interface called the studio. This is its page for the Acorn Tools identity after the second batch, with the three records the identity holds and the run that placed each:

![The studio's page for one identity. It is headed Acorn Tools, shows the identity's ID, and lists the records c-1, c-4 and c-2 with the run that placed each.](docs/studio-identity.png)

## Getting started

You need:

- Rust 1.89 or later, with `cargo`. [rustup](https://rustup.rs) installs both. A Rust older than 1.85 stops at `feature edition2024 is required` and does not say which version it wants.
- A C compiler, which the build uses for SQLite.
- `git` and `curl`.
- Linux or macOS. The commands on this page were run on Linux on x86-64 with Rust 1.100.0. Windows has not been tried.
- The network, during the build: Cargo downloads the crates, and the build downloads the DuckDB library it links against.

Build the binary and put it on the search path of this terminal:

```sh
git clone https://github.com/alexpatow/spool.git
cd spool
cargo build --release --locked -p spool-cli
export PATH="$PWD/target/release:$PATH"
spool --version
```

```text
spool 0.1.0
```

The build took two and a half minutes on four cores and left 1.2 GB in `target`. There is no install step. The binary loads the DuckDB library from the checkout's `target` directory, so leave the checkout where it is.

Scaffold a project outside the checkout and start the server:

```sh
cd ..
spool init companies --entity company --source crm
cd companies
spool dev
```

`spool init` says what it made and which files to edit. `spool dev` prints this and keeps running:

```text
project   .
store     ./.spool/dev.duckdb
applied   0348febc994b9f1fa46efc552547ce0b7a5c8c73fa88b81a4df2ca718c797414 (declared company)
accounts  ./.spool/dev.auth.sqlite
listening on http://127.0.0.1:7765
studio    http://127.0.0.1:7765/
mcp       http://127.0.0.1:7765/mcp
```

`spool dev` listens on this machine only and asks nobody to sign in. Its data is in `.spool/dev.duckdb` inside the project, and it applies changes to the project's files as you save them.

Leave it running. Open a second terminal in the directory that holds `spool` and `companies`:

```sh
export PATH="$PWD/spool/target/release:$PATH"
cd companies
```

Now run the commands of the [example](#example) in that terminal. There is no command that loads rows or starts a run yet, which is why the example uses `curl`. The studio is at <http://127.0.0.1:7765/>.

Ctrl-C in the first terminal stops the server. The data stays, and `spool dev` picks it up the next time. To start from nothing, stop the server and delete `companies/.spool`.

Not needed for the above: [`uv`](https://docs.astral.sh/uv/) and [`bun`](https://bun.sh). A project can judge pairs with your own code in place of SQL rules, which Spool calls a strategy. A strategy in Python runs with `uv` and one in TypeScript with `bun`, and nothing looks for either until such a file runs.

## What it is not for

These follow from the design and are not planned to change. Where something else fits better, it is named. What is said about other tools comes from reading their code and documentation in November 2026, not from running them beside Spool: see the [survey](docs/landscape-survey.md).

- **Deduplicating one file once.** Spool exists to keep IDs the same as data arrives over time. If you have one file, will get no more, and nobody stores the IDs, [Splink](https://github.com/moj-analytical-services/splink) is less to run: a Python library that scores pairs with a statistical model and needs no labelled examples. Its cluster IDs belong to one run and change when the data does.
- **Matching without rules of your own.** The built-in matcher runs SQL conditions that you write, which can use DuckDB's functions for similar strings. There is no trained model in it. For a matcher that learns from pairs you label, look at [dedupe](https://github.com/dedupeio/dedupe), or [Zingg](https://github.com/zinggAI/zingg) on Spark. Neither keeps identities between runs in its open-source form. A model can also judge pairs for Spool as a strategy, and Spool keeps the identities: `benchmarks/resolution/people` has one that runs Splink.
- **A decision per record, inside the request.** Rows are loaded in batches and matched by runs. Loading a row does not answer whether it is a duplicate. [Senzing](https://senzing.com), which is proprietary, resolves each record as it arrives. Its entity IDs can move as data loads.
- **Exact matching on a key you trust.** If two rows are the same thing whenever their tax numbers are equal, a join in your database is enough.
- **Data that is already in the FollowTheMoney format**, as sanctions and due-diligence data often is. [nomenklatura](https://github.com/opensanctions/nomenklatura), the Python library behind OpenSanctions, keeps stable IDs for people and companies in that format.
- **Streaming.** There is no connection to Kafka or to a change feed. Something else has to collect rows and send them.
- **More than one machine, or more than one writer.** One server process owns one DuckDB file and uses the cores and memory of one machine. There is no replication and no clustering.
- **Several tenants in one server.** A store holds one project. Separate customers need separate servers.
- **Embedding in your process.** There are no Python or Node bindings. Your code calls the server over HTTP.
- **Golden records.** Spool says which records are one thing. It does not produce one merged record with the best name and address, and it writes nothing back to a source.

## What it does not do yet

- **No release.** No package and no container image is published. The binary you build loads a library from its checkout, so the binary alone cannot be copied to another machine.
- **No command loads rows or starts a run.** Both are HTTP calls, in JSON or Arrow. There are no connectors: Spool does not read a database, a warehouse or a file store by itself.
- **A record cannot be deleted.** Once loaded it stays, so there is no way to remove a person's data on request.
- **No client library is published.** A TypeScript client is in `packages/client` and is not on npm. There is no Python client.
- **No command corrects anything.** The command line shows and explains. Merging, splitting, undoing and deciding a review item are HTTP calls or buttons in the studio.
- **Nothing is pushed when an identity changes.** A consumer reads the log of changes from the position it last saw.
- **Access is all or nothing.** A token has full access to the API and does not expire, though it can be revoked. People sign in with a password or with Google, and with no other identity provider.
- **No backup command and no migrations.** The format of the store can change before the first release, and a store in another format is refused.
- **Strategies need `uv` or `bun` on the machine**, and nothing looks for either until one runs.
- **Platforms.** CI tests on Linux on x86-64. Windows has not been tried.
- **Size.** The largest set it has been measured on is 5,000,000 records: 210 seconds and 13.4 GB of memory on four cores, matching the whole set from nothing. [benchmarks/README.md](benchmarks/README.md) has the measurements and how they were made. A request to the API may be 256 MiB at most.

## Status and licence

Version 0.1.0, not released, so there is no changelog yet.

MIT. See [LICENSE](LICENSE).

## Where next

- `spool` with no arguments lists the commands, and `spool <command> --help` describes each.
- [benchmarks/README.md](benchmarks/README.md): projects with more sources and rules than the one above, on public data, and what was measured on them.
- [docs/architecture-proposal.md](docs/architecture-proposal.md): the design. What records, judgements, identities, constraints and changesets are, how a project directory is laid out, and how strategies run.
- [docs/landscape-survey.md](docs/landscape-survey.md): the other tools that were read, and where Spool differs.
- [docs/sign-in.md](docs/sign-in.md): running a server that other people sign in to.
- [docs/mcp.md](docs/mcp.md): pointing an assistant that speaks the Model Context Protocol at the server.
- Bugs and questions: [open an issue](https://github.com/alexpatow/spool/issues).
