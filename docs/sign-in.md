# Signing people in

`spool serve` authenticates every caller. This page is for whoever runs the server: how people sign in, and what to set up before a team depends on it. The design is in the [architecture proposal](architecture-proposal.md#access).

## Passwords are for getting started

A new server prints a setup link on startup. Whoever opens it creates the first account, with a username and password, and becomes the admin who adds everyone else. No setup is needed first.

For a server a team depends on, use an identity provider instead. It handles offboarding, second factors and password resets, and saves people another password. Spool supports Google.

## Sign-in with Google

With Google set up, the sign-in page shows a "Sign in with Google" button above the password form. Google verifies who someone is, and the admin decides who gets in: an admin adds each person by their Google account email first, and anyone else is turned away.

### 1. Decide the address people open

Google redirects the browser back to the server after sign-in, so the server needs its public address, such as `https://spool.example.com`. It must be HTTPS unless it's on the server's own machine.

### 2. Make an OAuth client in Google's console

In the [Google Cloud console](https://console.cloud.google.com/), in a project of your organisation:

1. Set up the OAuth consent screen if the project has none. For a Google Workspace organisation, choose the audience "Internal" to admit only the organisation's accounts.
2. Under Credentials, create an "OAuth client ID" of the type "Web application".
3. Add one authorised redirect URI: the address from step 1 plus `/auth/google/callback`, such as `https://spool.example.com/auth/google/callback`. The server prints the exact value on startup, on the line starting with `google`.
4. Copy the client ID and client secret.

Spool requests only the `openid` and `email` scopes.

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

Set both of the first two or neither: the server won't start with just one. The secret is read from the environment and never passed to strategies or extractors. When the server listens on localhost only, the public URL defaults to the listen address.

### 4. Add people by email

The first account still comes from the setup link and a password. That admin then adds each person on the studio's People page by their Google account email, which must be Gmail or Google Workspace. Passwords are optional once Google is set up, so most people won't need one. Add the admin's own email the same way.

An account can have an email, a password, or both, and its owner can sign in with either.

## Details

- **The email must be one Google is authoritative for**: Gmail or Google Workspace. Google accounts registered with another provider's address are refused, because that address may have changed hands since Google verified it. People with such addresses use a password instead.
- **An account binds to the Google account that first signs in with its email**, by Google's own ID rather than the address. If the address is later reassigned, the new owner is refused. To move an account to another Google account, an admin changes its email, which also ends its Google sessions everywhere.
- **Disabling an account ends its access**, however it signed in. Removing someone in Google stops new sign-ins, and an existing session lasts at most twelve hours unless an admin disables the account.
- **Existing passwords keep working** alongside Google. To rely on Google alone, add people without passwords. Passwords can be replaced but not removed.
- **If Google is unreachable**, the server still starts and passwords still work. Google sign-in recovers when Google does.
- **`spool dev` has no sign-in**, so it ignores these settings.

## Trying it without Google

`bun run demo --google` in `packages/studio` starts a server with a local Google stand-in. Its sign-in page just asks which email to use, so you can try the whole flow before creating a client. With `SPOOL_GOOGLE_CLIENT_ID` and `SPOOL_GOOGLE_CLIENT_SECRET` set, the same command uses real Google.
