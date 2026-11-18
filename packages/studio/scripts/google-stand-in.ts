// Something that stands in for Google on this machine, so that sign-in with
// Google can be tried and tested without a client made in Google's console.
// It speaks OpenID Connect as Google does: it says where its sign-in is,
// sends the browser back with a code, and exchanges the code for a token it
// signed. Where Google asks for a password, it asks only which email to
// sign in as. It is for development and nothing else.

import { createHash, randomBytes } from "node:crypto";

export interface StandIn {
  /** Where it listens: what the server is told Google's sign-in is. */
  issuer: string;
  /** What `spool serve` is started with to be its client. */
  environment: Record<string, string>;
  stop(): Promise<void>;
}

interface Grant {
  email: string;
  /** The organisation whose Google Workspace the account is of, if it is of one. */
  hosted: string | undefined;
  nonce: string;
  challenge: string;
}

const base64url = (bytes: ArrayBuffer | Uint8Array | string) => Buffer.from(bytes as ArrayBuffer).toString("base64url");

const page = (title: string, body: string) =>
  new Response(
    `<!doctype html><meta charset="utf-8"><title>${title}</title>
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:28rem;margin:4rem auto;padding:0 1rem}
input,button{font:inherit;padding:.4rem .6rem}input[type=email]{width:100%;box-sizing:border-box;margin:.5rem 0 1rem}</style>
<h1>${title}</h1>${body}`,
    { headers: { "Content-Type": "text/html; charset=utf-8" } },
  );

const escaped = (text: string) => text.replace(/[&<>"']/g, (char) => `&#${char.charCodeAt(0)};`);

/** Whether an address is on this machine: the stand-in sends a browser nowhere else. */
function local(address: string): boolean {
  try {
    return ["127.0.0.1", "localhost", "[::1]"].includes(new URL(address).hostname);
  } catch {
    return false;
  }
}

export async function standInForGoogle(port = 0): Promise<StandIn> {
  const clientId = "spool-on-this-machine";
  const clientSecret = randomBytes(24).toString("hex");
  const keys = await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"],
  );
  const { n, e } = await crypto.subtle.exportKey("jwk", keys.publicKey);
  const grants = new Map<string, Grant>();

  const server = Bun.serve({
    port,
    hostname: "127.0.0.1",
    async fetch(request) {
      const url = new URL(request.url);
      const issuer = url.origin;
      const asked = url.searchParams;
      switch (url.pathname) {
        case "/.well-known/openid-configuration":
          return Response.json({
            issuer,
            authorization_endpoint: `${issuer}/authorize`,
            token_endpoint: `${issuer}/token`,
            jwks_uri: `${issuer}/jwks`,
            response_types_supported: ["code"],
            subject_types_supported: ["public"],
            id_token_signing_alg_values_supported: ["RS256"],
          });
        case "/jwks":
          return Response.json({ keys: [{ kty: "RSA", use: "sig", alg: "RS256", kid: "stand-in", n, e }] });
        case "/authorize": {
          // Where Google asks who you are and for your password.
          const kept = ["redirect_uri", "state", "nonce", "code_challenge"]
            .map((name) => `<input type="hidden" name="${name}" value="${escaped(asked.get(name) ?? "")}">`)
            .join("");
          return page(
            "This is not Google",
            `<p>It stands in for Google on this machine, so that signing in with Google can be tried.
Type the email of the Google account to sign in as. Spool lets in an email that an admin gave an account.</p>
<form action="/approve">${kept}<label>Email<input type="email" name="email" required autofocus></label>
<p><label><input type="checkbox" name="personal"> A Google account someone made for themselves with this address.
Google answers for whose an address is only if it is Gmail's or an organisation's Google Workspace, so Spool refuses this one.</label></p>
<button>Sign in as this account</button></form>`,
          );
        }
        case "/approve": {
          const back = asked.get("redirect_uri") ?? "";
          const email = (asked.get("email") ?? "").trim().toLowerCase();
          if (!local(back) || email === "") {
            return new Response("the stand-in sends a browser back to this machine only, as someone", { status: 400 });
          }
          const code = randomBytes(16).toString("hex");
          // As Google has it: an account of an organisation names it, and one of Gmail or of nobody does not.
          const domain = email.slice(email.lastIndexOf("@") + 1);
          const hosted = asked.has("personal") || domain === "gmail.com" ? undefined : domain;
          const [nonce, challenge] = [asked.get("nonce") ?? "", asked.get("code_challenge") ?? ""];
          grants.set(code, { email, hosted, nonce, challenge });
          const to = new URL(back);
          to.searchParams.set("code", code);
          to.searchParams.set("state", asked.get("state") ?? "");
          return Response.redirect(to.toString(), 302);
        }
        case "/token": {
          const refused = (error: string) => Response.json({ error }, { status: 400 });
          const client = `Basic ${Buffer.from(`${clientId}:${clientSecret}`).toString("base64")}`;
          if (request.headers.get("authorization") !== client) return refused("invalid_client");
          const form = new URLSearchParams(await request.text());
          const code = form.get("code") ?? "";
          const grant = grants.get(code);
          // A code is good once.
          grants.delete(code);
          const proved = base64url(
            createHash("sha256")
              .update(form.get("code_verifier") ?? "")
              .digest(),
          );
          if (grant === undefined || proved !== grant.challenge) return refused("invalid_grant");
          const now = Math.floor(Date.now() / 1000);
          const head = { alg: "RS256", kid: "stand-in", typ: "JWT" };
          const claims = {
            iss: issuer,
            aud: clientId,
            // One account for each email, whatever happens to the address later.
            sub: `stand-in:${grant.email}`,
            email: grant.email,
            email_verified: true,
            hd: grant.hosted,
            nonce: grant.nonce,
            iat: now,
            exp: now + 300,
          };
          const signed = `${base64url(JSON.stringify(head))}.${base64url(JSON.stringify(claims))}`;
          const signature = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", keys.privateKey, Buffer.from(signed));
          return Response.json({
            access_token: "unused",
            token_type: "Bearer",
            id_token: `${signed}.${base64url(signature)}`,
          });
        }
        default:
          return new Response("nothing here", { status: 404 });
      }
    },
  });

  const issuer = `http://127.0.0.1:${server.port}`;
  return {
    issuer,
    environment: {
      SPOOL_GOOGLE_CLIENT_ID: clientId,
      SPOOL_GOOGLE_CLIENT_SECRET: clientSecret,
      SPOOL_GOOGLE_ISSUER: issuer,
    },
    stop: () => server.stop(true),
  };
}
