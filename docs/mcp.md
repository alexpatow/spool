# Connecting an assistant: the MCP endpoint

The Spool server answers the [Model Context Protocol](https://modelcontextprotocol.io) at `/mcp`, in the same process as the API and the studio. An assistant that is pointed at it can look things up in the store and change identities on someone's behalf.

```
$ spool serve --store spool.duckdb
listening on http://0.0.0.0:7765
studio    http://0.0.0.0:7765/
mcp       http://0.0.0.0:7765/mcp
```

## What it is

- **A tool is a call of the API.** Each tool runs the handler that the matching path under `/v1` runs, so it answers what the API answers and is refused what the API refuses. A changeset that breaks a rule is rejected whole, with the operation that broke it.
- **The caller is whoever the server says it is.** `/mcp` takes the callers `/v1` takes and no others: the bearer token, which writes as `agent:api`, or the session of someone signed in, which writes as `user:<id>`. No tool has an argument that names the author of a change, and a call that sends one is refused.
- **Every change says why.** The tools that write require a `reason`, which the log keeps beside the change. The API leaves it optional. An assistant's change with no reason is hard to review afterwards.
- **Nothing is kept between requests.** The transport is Streamable HTTP without sessions, and every answer is one JSON response. Each request is authorized on its own. The endpoint speaks the protocol revisions from `2024-11-05` to `2026-07-28`, so a client that starts with the `initialize` handshake and one that sends no handshake both work.
- **The protocol is the `rmcp` crate's**, the official Rust SDK. It is compiled into the binary: there is no second process and nothing else to install.

A failure is an answer the model can read: the result has `isError` set, and its text is the error the API gives, `{"error": {"code": ..., "message": ...}}`. Only a call of a tool that does not exist is an error of the protocol.

## The tools

Tools that look things up. Each is marked read-only for the client.

| Tool | What it answers | The same as |
| --- | --- | --- |
| `describe_store` | The entity types, their features and sources, counts, and `head` | `GET /v1/info` |
| `search_identities` | Identities by words in their records, by type, or by ID | `GET /v1/identities` |
| `get_identity` | One identity and its records | `GET /v1/identities/{id}` |
| `get_identity_history` | Every change that touched an identity | `GET /v1/identities/{id}/history` |
| `search_records` | Records by words, source, key or identity, with their values | `GET /v1/records` |
| `get_record` | One record | `GET /v1/records/{id}` |
| `explain_pair` | Why two records share an identity, or why they do not | `GET /v1/explain` |
| `list_review_items` | The cases left for someone to decide, the open ones by default | `GET /v1/review` |
| `get_review_case` | One review item with what is needed to decide it | `GET /v1/review/{id}/case` |
| `list_constraints` | The must-link and cannot-link decisions in force | `GET /v1/constraints` |
| `list_changesets` | The log after a position | `GET /v1/changesets` |
| `mint_identity_ids` | New identity IDs for a `create_identity` operation | `POST /v1/ids` |

Tools that change something. Each is marked as a change, so a client can ask the person first.

| Tool | What it does | The same as |
| --- | --- | --- |
| `commit_changeset` | Merges identities, moves or places records, makes identities, adds and removes constraints | `POST /v1/changesets` |
| `split_identity` | Divides one identity in two and keeps the sides apart | `POST /v1/identities/{id}/split` |
| `undo_changeset` | Takes back one committed changeset, or shows what would | `POST /v1/changesets/{seq}/undo` |
| `dismiss_review_item` | Closes a review item without placing its records | `POST /v1/review/{id}/dismiss` |

Not offered: ingesting rows and documents, starting and reconciling match runs, drafts, and people. Those are the work of a pipeline or an admin, and they stay on `/v1`.

Two limits keep an answer the size a model can use. A list returns 20 entries unless the call gives a `limit`, and a result over 256 KiB is refused with an error that says to ask for less.

## Adding it to a client

A client needs two things: the address, `https://<your server>/mcp`, and the server's token, sent as `Authorization: Bearer <token>`. What a client that cannot send a header needs is under [Clients that sign in with OAuth](#clients-that-sign-in-with-oauth).

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

In Claude, go to Customize, Connectors, Add custom connector, and enter the address. Claude calls the server from Anthropic's network, so the server needs a public HTTPS address.

Claude then needs a way to send the token. An organization whose Owner sees a **Request headers** section when adding the connector enters `Authorization` and `Bearer <token>` there. Anthropic describes that section as a beta for a limited set of organizations. Without it, a custom connector signs in with OAuth or not at all, and Spool is not an OAuth server: see below.

### ChatGPT

In ChatGPT on the web, go to Plugins, the plus button, Add custom MCP server. The authentication it offers is OAuth, no authentication, or both. There is no field for a token, so ChatGPT on the web cannot be pointed straight at a Spool server that asks for one: see below.

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

Claude on the web without the request-header beta, and ChatGPT on the web, add a remote server either with no sign-in or through OAuth 2.1: the client finds the server's authorization server, registers itself, sends the person to sign in and approve, and gets a token of its own. Spool has no authorization server. Asked for one, it answers `404`, and such a client reports that it could not connect.

What can be done about it, none of which is built:

1. **Spool becomes an OAuth authorization server.** The person signs in to Spool in the browser and approves the assistant, and what the assistant changes is recorded under that person. It is the only choice that works in both products with nothing else to run, and the most to build and to get right: metadata documents, client registration, the authorization code flow with PKCE, tokens and their refresh and revocation.
2. **An OAuth gateway in front of the server.** A proxy that speaks OAuth to the client, signs people in with an identity provider, and sends Spool's token with what it passes on. Nothing changes in Spool. Every change is recorded under the one token.
3. **For ChatGPT, OpenAI's Secure MCP Tunnel.** `tunnel-client` runs beside the server, and ChatGPT reaches it through OpenAI with the connection set to Tunnel and no authentication. `tunnel-client` can start a local command as the server, and `mcp-remote` with the header, as under Claude Desktop, is such a command. Who may use the tunnel is then decided in the OpenAI workspace.

## Where it listens

`spool serve` answers `/mcp` by whatever name the server is reached by. Like every write to the server, a request that a browser sends from another site's page is refused.

`spool dev` asks nobody to sign in, so its endpoint answers only when it is called by a name of this machine: `localhost`, `127.0.0.1` or `::1`. A page on another site that has a name of its own for this machine is refused. What an assistant changes on a development server is recorded under the `dev` account.

## What was tried

Against a server holding the company scenario of `benchmarks/`, in November 2026:

| Client | What was done | Result |
| --- | --- | --- |
| Claude Code 2.1.327 | `claude mcp add` with the header, then `claude mcp list` and `claude mcp get` | Connected, with a token and against `spool dev`. A wrong token and no token are each reported with the server's reason. No conversation was held, so no model called a tool. |
| MCP Inspector 2.16.0, command line | Listed the tools and called them | Worked |
| TypeScript SDK 1.32.2 | Connected, listed, searched, merged two identities, sent an `actor` | Worked on revision `2025-11-25`; the merge is in the log as `agent:api`; the `actor` was refused |
| Python SDK 2.3.0 | Connected, listed, read the log, undid the merge | Worked on revision `2026-07-28`, the one without a handshake |
| `mcp-remote` 0.16.6 | Called a tool through it from the Inspector, with the header | Worked. Claude Desktop itself was not run. |
| Codex CLI 0.170.0 | `codex mcp add` and `codex mcp list` | The configuration is accepted. Codex connects only in a session, which needs an account, so no connection was made. |
| Claude on the web, ChatGPT on the web, the two model APIs, Secure MCP Tunnel | Not tried | They need an account and a server on a public address. What is said about them above is from their documentation. |
