# Connecting an agent over MCP

The Spool server speaks the [Model Context Protocol](https://modelcontextprotocol.io) at `/mcp`, in the same process as the API and the studio. Agents connected to it can load rows and documents, run the project's extractors, match, look things up and change identities. The [MCP page of the docs](https://spool.alexpatow.com/docs/mcp) lists every tool with an example call. This page covers connecting each client.

```
$ spool serve --store spool.duckdb
accounts  spool.auth.sqlite
listening on http://0.0.0.0:7765
studio    http://0.0.0.0:7765/
mcp       http://0.0.0.0:7765/mcp
```

## How it works

- **Each tool wraps an API call.** It runs the same handler as the matching path under `/v1`, so it returns what the API returns and fails where the API fails.
- **The server decides who the caller is.** `/mcp` accepts the same callers as `/v1`: a token, which writes as `agent:<name>`, or a signed-in session, which writes as `user:<id>`. No tool lets the caller name a different author.
- **Changes carry a reason.** Tools that change identities or retract rows require a `reason`, which the log stores with the change. Loads, extractions and matches record who started them instead.
- **It's stateless.** The transport is Streamable HTTP without sessions, and every response is one JSON message. Supported protocol revisions run from `2024-11-05` to `2026-07-28`, so clients with and without the `initialize` handshake both work.
- **It's built in.** The protocol comes from `rmcp`, the official Rust SDK, compiled into the binary. There's nothing else to install.

Failures come back as results the model can read, with `isError` set and the API's error as text.

## Tokens

A client needs the address, `https://<your server>/mcp`, and a token sent as `Authorization: Bearer <token>`. Create one token per agent, named after it, so the log shows which agent made each change and you can revoke one without the others. An admin creates them on the studio's Tokens page, or on the server's machine:

```
$ spool token add claude --store spool.duckdb
```

That prints the token once. Changes are recorded as `agent:claude`. The examples below assume the token is in `SPOOL_TOKEN`.

Tokens have full API access, so mark the tools that change something as needing approval in clients that support it.

## Claude Code

```
claude mcp add --transport http spool https://spool.example.com/mcp \
  --header "Authorization: Bearer $SPOOL_TOKEN"
claude mcp list
```

`claude mcp list` shows `Connected` when the address and token are right. Against `spool dev`, drop the header: `claude mcp add --transport http spool http://127.0.0.1:7765/mcp`.

## Claude Desktop

Claude Desktop starts local servers from `claude_desktop_config.json` (Settings, Developer, Edit Config). `mcp-remote` is a local proxy that forwards every call to a remote server with a header:

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

It runs on your machine, so it also reaches servers on a private network.

## Codex

```
codex mcp add spool --url https://spool.example.com/mcp --bearer-token-env-var SPOOL_TOKEN
```

That writes this to `~/.codex/config.toml`:

```toml
[mcp_servers.spool]
url = "https://spool.example.com/mcp"
bearer_token_env_var = "SPOOL_TOKEN"
```

## From code

The MCP SDKs take the address and a header. So do model APIs that call MCP servers for you: the Claude API's MCP connector takes the token as `authorization_token`, and the OpenAI Responses API's `mcp` tool takes it as `authorization`. Both call from the vendor's network, so the server needs a public HTTPS address.

## Claude and ChatGPT on the web

Web clients add remote servers without auth or through OAuth 2.1. Spool authenticates with bearer tokens. In Claude, organisations with the **Request headers** beta can enter `Authorization` and `Bearer <token>` when adding a custom connector. For ChatGPT, OpenAI's Secure MCP Tunnel can run `mcp-remote` with the header, as in the Claude Desktop setup above.

## Where it listens

`spool serve` answers `/mcp` on whatever hostname the server is reached by. As with every write, requests a browser sends from another site's page are rejected.

`spool dev` has no sign-in, so it only answers requests sent to `localhost` or a loopback address such as `127.0.0.1`. Everything an agent does on a development server is recorded under the `dev` account.
