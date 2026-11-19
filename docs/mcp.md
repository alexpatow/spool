# Connecting an assistant: the MCP endpoint

The Spool server answers the [Model Context Protocol](https://modelcontextprotocol.io) at `/mcp`, in the same process as the API and the studio. An assistant that is pointed at it can load rows and documents, run the project's extractors and match what was loaded, look things up in the store, and change identities on someone's behalf.

```
$ spool serve --store spool.duckdb
accounts  spool.auth.sqlite
listening on http://0.0.0.0:7765
studio    http://0.0.0.0:7765/
mcp       http://0.0.0.0:7765/mcp
```

## What it is

- **A tool is a call of the API.** Each tool runs the handler that the matching path under `/v1` runs, so it answers what the API answers and is refused what the API refuses. A changeset that breaks a rule is rejected whole, with the operation that broke it.
- **The caller is whoever the server says it is.** `/mcp` takes the callers `/v1` takes, a token, which writes under its name as `agent:<name>`, or the session of someone signed in, which writes as `user:<id>`, and one more: an assistant that signed in with OAuth, which writes as the person who approved it. No tool has an argument that names the author of a change, and a call that sends one is refused.
- **A change made under the caller's name says why.** The tools that change identities or take rows back require a `reason`, which the log keeps beside the change. The API leaves it optional. An assistant's change with no reason is hard to review afterwards. A load, an extraction and a match run take no reason: a run records who started it, and what it did is in its report. See [Loading and matching](#loading-and-matching).
- **Nothing is kept between requests.** The transport is Streamable HTTP without sessions, and every answer is one JSON response. Each request is authorized on its own. The endpoint speaks the protocol revisions from `2024-11-05` to `2026-07-28`, so a client that starts with the `initialize` handshake and one that sends no handshake both work.
- **The protocol is the `rmcp` crate's**, the official Rust SDK. It is compiled into the binary: there is no second process and nothing else to install.

A failure is an answer the model can read: the result has `isError` set, and its text is the error the API gives, `{"error": {"code": ..., "message": ...}}`. Only a call of a tool that does not exist is an error of the protocol.

## The tools

Tools that look things up. Each is marked read-only for the client.

| Tool | What it answers | The same as |
| --- | --- | --- |
| `describe_store` | The entity types and their features, the sources the project declares with their columns, the extractors, counts, and `head` | `GET /v1/info` |
| `search_identities` | Identities by words in their records, by type, or by ID | `GET /v1/identities` |
| `get_identity` | One identity and its records | `GET /v1/identities/{id}` |
| `get_identity_history` | Every change that touched an identity | `GET /v1/identities/{id}/history` |
| `search_records` | Records by words, source, key or identity, with their values | `GET /v1/records` |
| `get_record` | One record | `GET /v1/records/{id}` |
| `explain_pair` | Why two records share an identity, or why they do not. A record is named by its ID, as `<source>:<key>`, or, for what an extractor found, by its place as `<source>:<document key>[<start>-<end>]` | `GET /v1/explain` |
| `list_review_items` | The cases left for someone to decide, the open ones by default | `GET /v1/review` |
| `get_review_case` | One review item with what is needed to decide it | `GET /v1/review/{id}/case` |
| `list_constraints` | The must-link and cannot-link decisions in force | `GET /v1/constraints` |
| `list_changesets` | The log after a position | `GET /v1/changesets` |
| `get_run` | One run: whether it is running, and what it reported | `GET /v1/runs/{run}` |
| `mint_identity_ids` | New identity IDs for a `create_identity` operation | `POST /v1/ids` |

Tools that change something. Each is marked as a change, so a client can ask the person first.

| Tool | What it does | The same as |
| --- | --- | --- |
| `load_rows` | Stores rows from one source as records | `POST /v1/load` |
| `load_documents` | Stores documents from one source as text, for an extractor to read | `POST /v1/documents` |
| `run_extraction` | Runs one of the project's extractors over the documents it has not read. What it finds is records | `POST /v1/extractions` |
| `run_match` | Places the records no run has examined, and commits the result | `POST /v1/match` |
| `retract_rows` | Takes rows back by their keys: they leave their identities and stay in the store | `POST /v1/retract` |
| `commit_changeset` | Merges identities, moves or places records, makes identities, adds and removes constraints | `POST /v1/changesets` |
| `split_identity` | Divides one identity in two and keeps the sides apart | `POST /v1/identities/{id}/split` |
| `undo_changeset` | Takes back one committed changeset, or shows what would | `POST /v1/changesets/{seq}/undo` |
| `dismiss_review_item` | Closes a review item without placing its records | `POST /v1/review/{id}/dismiss` |

Not offered: listing and reading documents, a match run that leaves a draft or that waits for the caller's own judgements, approving or discarding a draft, and people and tokens. Those stay on `/v1`.

Two limits keep an answer the size a model can use. A list returns 20 entries unless the call gives a `limit`, and a tool returns at most 256 KiB. A lookup over that is refused with an error that says to ask for less. A change over that was made all the same, so it is answered as made, without its large fields: the answer keeps where the change was committed and names what it left out in `left_out`, as in `changeset.ops`. A run is answered the same way by `run_match` and by `get_run`: a match run that left thousands of records for review keeps its counts and leaves out `run.resolution.review`, which `list_review_items` reads. A changeset rejected for more reasons than fit is answered with the first of them, and its message says how many there are.

## Loading and matching

`load_rows` takes the name of a source, an entity type, and the rows as JSON objects, and stores them as records. `run_match` then places every record of the type that no run has examined: each joins the identity it matches or gets a new one, and the identities that existed keep their IDs. Text is the other way in. `load_documents` takes the name of a source and documents as a `key` and a `text` each, and stores them. `run_extraction` then has one of the project's extractors read the documents it has not read, and each place where a document names something becomes a record of the document's source. Those records are placed by `run_match` as rows are.

Each of the four answers with the run, and a run that is still going after 30 seconds is answered as `running`, to be read with `get_run`.

- **The project decides what can be loaded.** A source the project does not declare is refused, and so is a column the source does not list. `describe_store` lists the project's sources in `sources`, each with its columns and the entity types it sends rows of, before any row is loaded.
- **Documents of a source the project does not declare are refused.** Here the tool is stricter than `POST /v1/documents`, which stores a document under any source name and then refuses the records an extractor finds in it. A document cannot be deleted, so an assistant that got the name wrong would leave one that gives nothing.
- **An empty value is one the row does not have.** `load_rows` reads `""` as it reads `null` and a column left out of the row, which is how `spool load` and a CSV body read an empty field. Here the tool differs from `POST /v1/load` with rows as JSON, which stores `""` as text. A rule that compares the column then says nothing about the row, where empty text would differ from every other value.
- **Loading the same rows or documents twice adds nothing.** A document with a key the store knows and other text becomes the new version of that document, and the earlier version is kept. A text it held before is one more version of it.
- **`load_rows` does not replace a row unless it is told to.** A row with a key the store holds and other values would become the new version of that row. The tool refuses it, and the whole call with it: nothing is loaded, and the error names the keys. Here the tool differs from `POST /v1/load`, which replaces a row unless the call has `replace` set to `false`: a pipeline sends a row because it changed, and an assistant that sends other values under a known key has more often got the rows wrong. With `replace` set to `true` the tool stores the rows as new versions, the earlier versions are kept, and the run counts them in `superseded`. Values the row held before count as other values: loading them changes the row back, as one more version, and the versions between are kept. So an older file loaded after a newer one would move the rows the newer one changed back to what the older one says, and the tool refuses that too unless it is told to replace.
- **`run_extraction` runs the project's code, and nothing a caller sends.** `describe_store` lists the extractors in `extractors`, and a project with only one need not name it, as with `spool extract`. An extractor may ask a language model, which takes time and can cost money, so an assistant that may call this tool may spend it. `limit` says how many documents a call reads at most.
- **Nothing deletes a loaded row or a stored document.** `retract_rows` takes rows back by their keys: every version of each leaves its identity, no later run examines it, and it stays in the store marked as retracted. Loading the row again under the same key ends the retraction. A record an extractor found is taken back the same way, by the key `search_records` shows for it. No tool takes a document back. `undo_changeset` takes back what a match run committed: the records stay, without an identity, and `run_match` with `rematch` places them again under new identity IDs.
- **A call is 4 MiB at most**, which is a few thousand rows. A pipeline loads more with `POST /v1/load`, which takes 256 MiB and Arrow, and `POST /v1/documents`.
- **`run_match` always commits.** It takes no `commit` argument, because no tool approves a draft. Its `strategy` is `rules` or one the project declares for the type. Any other name is refused: on the API such a run waits for the caller's judgements, and no tool sends them.
- **A token can do everything a tool does.** There are no tokens that only read, so an assistant that may look things up may also load, match, retract and merge. Give an assistant a token only on a store it may change, and mark the tools that change something as needing approval in a client that can ask.
- **A run records who started it.** A load, an extraction and a match run each keep the caller that asked for them, as `started_by`: `get_run` gives it, and so do `GET /v1/runs`, `spool show runs` and the studio. What a match run commits is still the engine's in the log, with the reason "reconcile new records", because the engine decided those placements and the caller only asked for the run. The log entry names both: its `actor` is `engine` and its `started_by` is the caller. A retraction, and every changeset an assistant commits, is recorded under its name with its reason.

## Adding it to a client

A client needs two things: the address, `https://<your server>/mcp`, and a token, sent as `Authorization: Bearer <token>`.

Make a token for each assistant, named after it, so that the log says which one made a change and one can be revoked without the others. An admin makes one on the studio's Tokens page, and on the server's machine:

```
$ spool token add claude --store spool.duckdb
```

prints the token once. What that assistant changes is recorded as `agent:claude`. The examples below have the token in `SPOOL_TOKEN`.

What a client that cannot send a header needs is under [Clients that sign in with OAuth](#clients-that-sign-in-with-oauth).

### Claude Code

```
claude mcp add --transport http spool https://spool.example.com/mcp \
  --header "Authorization: Bearer $SPOOL_TOKEN"
claude mcp list
```

`claude mcp list` says `Connected` when the address and the token are right. Against `spool dev` leave the header out: `claude mcp add --transport http spool http://127.0.0.1:7765/mcp`.

### Claude Desktop

Claude Desktop starts local servers from `claude_desktop_config.json` (Settings, Developer, Edit Config). The `mcp-remote` package is a local server that passes every call on to a remote one, with a header:

```json
{
  "mcpServers": {
    "spool": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "https://spool.example.com/mcp", "--header", "Authorization:${SPOOL_AUTH}"],
      "env": { "SPOOL_AUTH": "Bearer <token>" }
    }
  }
}
```

This runs on the person's own machine, so it also reaches a server on a private network.

### Claude on the web, and connectors added in Claude Desktop

In Claude, go to Customize, Connectors, Add, Add custom connector, and enter the address. Under OAuth client, choose **Register automatically**. Claude calls the server from Anthropic's network, so the server needs a public HTTPS address on port 443. Claude then signs in with OAuth: see [Clients that sign in with OAuth](#clients-that-sign-in-with-oauth).

An organization whose Owner sees a **Request headers** section when adding the connector can instead enter `Authorization` and `Bearer <token>` there, and the connector works as that token. Anthropic describes that section as a beta for a limited set of organizations.

### ChatGPT

In ChatGPT on the web, go to Plugins, the plus button, Add custom MCP server. Enter the address, choose OAuth, and create it as a plugin. ChatGPT signs in with OAuth: see [Clients that sign in with OAuth](#clients-that-sign-in-with-oauth).

### Codex

```
codex mcp add spool --url https://spool.example.com/mcp --bearer-token-env-var SPOOL_TOKEN
```

which writes this to `~/.codex/config.toml`:

```toml
[mcp_servers.spool]
url = "https://spool.example.com/mcp"
bearer_token_env_var = "SPOOL_TOKEN"
```

### From code

The MCP SDKs take the address and a header. So do the model APIs that call an MCP server for you: the MCP connector of the Claude API takes the token as `authorization_token`, and the `mcp` tool of the OpenAI Responses API takes it as `authorization`. Both call from the vendor's network and need a public HTTPS address.

## Clients that sign in with OAuth

Claude and ChatGPT on the web cannot be given a token. They add a remote server through OAuth 2.1, as the Model Context Protocol's authorization specification says, and the server is the OAuth authorization server of its own `/mcp`:

1. A call of `/mcp` without a token is answered `401`, with `WWW-Authenticate: Bearer resource_metadata="<address>/.well-known/oauth-protected-resource/mcp"`.
2. That document (RFC 9728) names `<address>/mcp` as the resource and the server as its authorization server, whose metadata is at `/.well-known/oauth-authorization-server` (RFC 8414).
3. The client registers itself at `/oauth/register` (RFC 7591), as a public client that proves itself with PKCE. Registration is open: a client is a name and the addresses its approvals may go to, HTTPS or a loopback address. One nobody approved is forgotten after a day.
4. It sends the person to `/oauth/authorize` with a PKCE challenge (S256 only) and `resource`. That address opens the studio, which signs them in if need be, the usual way, and shows what asks, the host the approval goes back to, and a warning when that host is the person's own machine. They approve or turn it away.
5. The browser goes back to the client with a code, `state`, and `iss` (RFC 9207). The client exchanges the code at `/oauth/token` for an access token and a refresh token.
6. It calls `/mcp` with the access token, as the person who approved it. What it changes is recorded under them, as `user:<id>`.

| What | How long |
| --- | --- |
| A code | 2 minutes, used once. A second use ends what the first gave |
| An access token | 1 hour. It works at `/mcp` and is refused at `/v1` |
| A refresh token | 30 days. Each is used once and gives the next; one used twice ends the approval |
| An approval | As long as the sign-in it was given in would last, without the 12 hours: until the account is turned off, the password it was given with is replaced, or, for Google, the account's email changes. It stays ended if the account is turned on again: the assistant is approved anew |

An admin sees every approval under Assistants on the studio's Tokens page, with who gave it and when it was last used, and revokes one there. A client can revoke its own tokens at `/oauth/revoke` (RFC 7009). There is one scope, `spool`: everything a token may do.

The issuer is the address the server is reached by: `--public-url`, on Render the address Render gives the service, or the address it listens on when that is on this machine only. A server that knows no such address, whose address is plain HTTP and not on this machine, or that runs without the studio, has none of this: it prints `oauth     off` and the reason when it starts, and answers the paths above with `404`. `spool dev` has none either; it asks nobody to sign in.

Clients are registered dynamically only. Client ID Metadata Documents, which the specification now prefers and which Claude offers as "Claude's published identity", are not supported yet: they have the server fetch an address the client names.

## Where it listens

`spool serve` answers `/mcp` by whatever name the server is reached by. Like every write to the server, a request that a browser sends from another site's page is refused.

`spool dev` asks nobody to sign in, so it answers, at `/mcp` as at every other path, only a request sent to `localhost` or to a loopback address such as `127.0.0.1`. A page on another site that has a name of its own for this machine is refused. What an assistant does on a development server is recorded under the `dev` account: it is the author of a changeset or a retraction, and the one who started a load, an extraction or a match run.

## What was tried

Against a server holding the company scenario of `benchmarks/`, in November 2026:

| Client | What was done | Result |
| --- | --- | --- |
| Claude Code 2.1.327 | `claude mcp add` with the header, then `claude mcp list` and `claude mcp get` | Connected, with a token and against `spool dev`. A wrong token and no token are each reported with the server's reason. No conversation was held, so no model called a tool. |
| MCP Inspector 2.16.0, command line | Listed the tools and called them | Worked |
| TypeScript SDK 1.32.2 | Connected, listed, searched, merged two identities, sent an `actor` | Worked on revision `2025-11-25`; the merge is in the log under the token's name; the `actor` was refused |
| Python SDK 2.3.0 | Connected, listed, read the log, undid the merge | Worked on revision `2026-07-28`, the one without a handshake |
| `mcp-remote` 0.16.6 | Called a tool through it from the Inspector, with the header | Worked. Claude Desktop itself was not run. |
| Codex CLI 0.170.0 | `codex mcp add` and `codex mcp list`, then `codex exec --oss` with a model run by Ollama | The configuration is accepted, and in a session Codex connects and lists the tools. With `qwen2.5:7b` it called them, with rows it made up. With `qwen3:8b` it called none. No session was held with an OpenAI model, which needs an account. |
| MCPHost 0.34.0 | Sessions with `qwen2.5:14b` run by Ollama 0.40.16 | Connected, and the model loaded rows, matched and explained a pair: see [A session with a small local model](#a-session-with-a-small-local-model) |
| An OAuth client driven by the tests | Registered, had a person approve it in the studio, exchanged the code, called `/mcp`, refreshed, and was revoked | Worked, against `spool serve` on this machine |
| Claude on the web, ChatGPT on the web, the two model APIs, Secure MCP Tunnel | Not tried | They need an account and a server on a public address. What is said about them above is from their documentation. |

## A session with a small local model

The steps of Getting started, done by a model over `/mcp` in November 2026: load a sample file, match it, and say why two rows are different companies.

**These sessions are a small local model's, and say nothing about Claude, Codex or ChatGPT.** No hosted model was called. The model is a 14-billion-parameter one that ran on the machine the server ran on. It is far weaker than the models those assistants use, and none of them has been run through these steps here. What follows shows that the tools can be driven from end to end, and what a refusal and a mistake look like.

- **The project** is the sample one, which `spool init companies --example companies` writes, served by `spool dev`, with `samples/crm-1.csv` loaded and matched before the session.
- **The client** is [MCPHost](https://github.com/mark3labs/mcphost) 0.34.0. It gives the model Spool's tools and a file reader of its own that reads only the project's directory. Spool has no tool that reads a file: the model reads it and sends the rows.
- **The model** is `qwen2.5:14b`, run by Ollama 0.40.16 on four processor cores, at temperature 0.

```json
{
  "mcpServers": {
    "files": {
      "type": "builtin",
      "name": "fs",
      "options": { "allowed_directories": ["/path/to/companies"] },
      "allowedTools": ["read_file", "list_directory"]
    },
    "spool": { "type": "remote", "url": "http://127.0.0.1:7765/mcp" }
  }
}
```

```
$ mcphost --config mcphost.json -m ollama:qwen2.5:14b --temperature 0 \
    -p "Load samples/crm-2.csv into Spool, match it, and tell me why c-5 and c-6 are different companies."
```

### When it went right

Four calls, and none was refused:

| Call | Arguments | Answer |
| --- | --- | --- |
| `read_file`, the client's | `samples/crm-2.csv` | The file's three rows |
| `load_rows` | `source: "crm"`, `entity_type: "company"`, and the three rows as objects | A load run: 3 received, 3 inserted |
| `run_match` | `entity_type: "company"` | A match run: 3 new records, 4 pairs judged a match, 1 identity created |
| `explain_pair` | `a: "crm:c-5"`, `b: "crm:c-6"` | The explanation |

The model sent the last three in one step, before it had the answer of the first.

Its answer, in part: "The reason these records were treated as different companies is due to the rule `different_country` in the matching strategy, which holds when the countries are different." That is what the explanation says. The model guessed the source and the entity type from the file's name and never called `describe_store`.

### What went wrong in the other sessions

The same prompt was given four times, each on a store that held only the rows of `samples/crm-1.csv`, and went right once. A session with another prompt and one with a document are listed with them. All six were held while the tools were being written: the answers then said `ingest` where they now say `load`, the description of `load_rows` did not yet say what `superseded` counts, and the session that went right had an earlier wording of the descriptions and of two errors than the other five, on a copy of the project that was written by hand.

| Session | What the model did | What the store held afterwards |
| --- | --- | --- |
| The prompt above | The four calls above | The three rows, placed |
| The prompt above | Loaded the three rows and matched them. Then, looking for `c-5` and `c-6`, it searched for names it made up and loaded two rows of its own under those keys | The three rows, placed. Both Halden Group rows replaced by invented ones |
| The prompt above | Did not read the file. Loaded six rows it made up, with the keys `c-1` to `c-6` | None of the file's rows. Five of the six rows of `crm-1.csv` replaced by invented ones |
| The prompt above | Wrote out rows it "assumed" the file held and stopped, with no call | Nothing changed |
| "Load samples/crm-1.csv into Spool and match it. How many companies are there, and which rows are the same company?", on an empty store | Called `load_rows` with the source `crm-1` and no rows, which was refused with the name of the source to use. Read the file, loaded its six rows, matched, read the four identities, and answered rightly | The six rows, placed |
| "Load samples/notes-1.txt into Spool as a document, run the extractor over it and match." | Did not read the file. Stored a text it made up under the file's name. Asked for an extractor named `default`, was refused with the name of the project's one, and ran that, which found nothing | A document that is not the file's, for good |

The session with a document ran on a project with a source for notes and an extractor added by hand, because `spool init` did not write them then. Its example `companies` has both now, with another extractor: a Python file that asks a small model.

What these sessions show about the tools, whatever the model:

- **A wrong name costs one call.** Each refusal names what to send instead: the sources a type takes rows from, the project's extractors, the form of an ID. The model's next try was right every time.
- **A row that is made up is stored like any other.** Spool cannot tell an invented row from a real one. Under a new key it is one more record, which `retract_rows` takes back. Under a key the store holds it becomes the new version of that row, in the identity the old version is in. Wherever that new version is placed afterwards, by a match or by `commit_changeset`, the earlier versions move along, as operations of the same changeset.
- **A row that was replaced is put back by loading it again.** When these sessions ran it was not: the store held the row's content as the earlier version, the load counted it as a duplicate, and the invented version stayed the one `crm:c-5` names. Now the row loaded from the file is one more version, which `crm:c-5` names, in the identity the row was in. That load replaces a row, so `load_rows` asks for `replace` set to `true`. The invented version stays in the row's history and in that identity, as every version does: the versions of a row go where its latest version is placed. `retract_rows` takes back every version of the key, the real one with it.
- **These sessions are why `load_rows` now refuses to replace a row.** When they were held it replaced one without a word: its answer counted the rows in `superseded`, and the model did not look at it. Both sessions that lost real rows sent invented ones under keys the store held, which the tool now refuses unless the call has `replace` set to `true`. No session with this model has been held since.
- **The model is asked to copy.** The descriptions say to read the file first and never to make a row up, and this model does it some of the time. A person who checks each change before it is made, as a client can ask for with these tools, is what stands between such a model and the store.

A smaller model, `qwen2.5:7b`, made rows up in every session.
