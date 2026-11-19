# Signing people in

A server run with `spool serve` always asks who a caller is. This page is for whoever runs one: how people sign in, and what to set up before others depend on it. The design is in the [architecture proposal](architecture-proposal.md#access).

## Passwords are for getting started

A new server prints a link when it starts. Whoever opens it makes the first account, with a username and a password, and is the admin who adds everyone else. Nothing needs to be set up first, which is what passwords are for here.

They are not what a server that people depend on should rest on:

- Someone who leaves the organisation keeps their access until an admin remembers to turn the account off.
- Spool has no second factor, no password reset by email and no rule about how often a password changes.
- Every password is one more secret for people to reuse or lose.

An identity provider already answers each of those. In production, sign people in through one. Spool supports Google today.

## Sign-in with Google

With Google set up, the sign-in page has a "Sign in with Google" button above the password form. Google says who someone is. Whether they have an account stays the admin's decision: an admin adds a person by the email of their Google account first, and anyone else who signs in with Google is refused.

### 1. Decide the address people open

Google sends the browser back to the server after signing in, so the server has to know the address it is reached by, such as `https://spool.example.com`. It must be HTTPS unless it is on the server's own machine.

### 2. Make an OAuth client in Google's console

In the [Google Cloud console](https://console.cloud.google.com/), in a project of your organisation:

1. Set up the OAuth consent screen if the project has none. For a Google Workspace organisation, choose the audience "Internal", which lets in only accounts of the organisation.
2. Under Credentials, create an "OAuth client ID" of the type "Web application".
3. Add one authorised redirect URI: the address from step 1 followed by `/auth/google/callback`, such as `https://spool.example.com/auth/google/callback`. The server prints the exact value when it starts, on the line that begins with `google`.
4. Keep the client ID and the client secret it shows.

Spool asks Google for the scopes `openid` and `email` and nothing else.

### 3. Start the server with the client

| Setting | What it is |
| --- | --- |
| `SPOOL_GOOGLE_CLIENT_ID` | The client ID from step 2 |
| `SPOOL_GOOGLE_CLIENT_SECRET` | The client secret from step 2 |
| `SPOOL_PUBLIC_URL`, or `--public-url` | The address from step 1 |

```sh
SPOOL_GOOGLE_CLIENT_ID=... \
SPOOL_GOOGLE_CLIENT_SECRET=... \
spool serve --public-url https://spool.example.com
```

Set both of the first two or neither: the server does not start with one. The secret is read from the environment and is not passed on to strategies or extractors. When the server listens on this machine only, the public URL may be left out, and is then the address it listens on.

### 4. Add people by email

The first account is still made with the link and a password. That admin then adds each person on the studio's People page with the email of their Google account, which must be a Gmail or Google Workspace address. A password is optional once Google is set up, so most people need none. Give the admin's own account its email the same way.

A person may have an email, a password, or both. An admin who adds an email to an account that has a password lets its owner use either.

## What to know

- **The email must be one Google answers for**: a Gmail address, or an address of an organisation's Google Workspace. Anyone can make a Google account with an address of another provider. Google checked such an address once, and it may have changed hands since, so a Google account like that is refused: someone who used to have the address could otherwise take the account meant for whoever has it now. A person with such an address needs a password, or a Google account of the first two kinds.
- **An account is bound to the Google account that first signs in with its email.** Afterwards Spool knows that Google account by Google's own ID for it, not by the address. If the address is later given to someone else, they are refused. To move an account to another Google account, an admin changes its email, which also signs the account out everywhere it signed in with Google. Those sessions stay ended even if the same Google account signs in again under the new address.
- **Turning an account off ends its access**, whichever way it signed in. Removing someone from the organisation in Google stops them signing in again; a session they already have lasts until it ends, at most twelve hours, or until an admin turns the account off.
- **Passwords that exist keep working.** Google is added beside them. To rely on Google alone, add people without passwords. An existing password cannot be removed yet, only replaced.
- **If Google cannot be reached**, the server still starts and passwords still work. Signing in with Google says so and works again when Google does.
- **`spool dev` has no sign-in**, so it ignores these settings.

## Assistants that sign in as a person

Claude and ChatGPT on the web connect to `/mcp` with OAuth: the assistant sends someone to the studio, they sign in as above and approve it, and it works as them until the approval ends. An approval lasts as long as the sign-in it was given in would, without the twelve hours: turning the account off, replacing the password it was given with, or changing the account's email for Google ends it, and it stays ended when the account is turned on again. An admin revokes any approval under Assistants on the Tokens page. [The MCP page](mcp.md#clients-that-sign-in-with-oauth) has the details.

## Trying it without Google

`bun run demo --google` in `packages/studio` starts a server with a stand-in for Google on this machine. Its sign-in page asks only which email to sign in as, so the whole path can be tried before a client exists. With `SPOOL_GOOGLE_CLIENT_ID` and `SPOOL_GOOGLE_CLIENT_SECRET` set, the same command uses Google.
