// The studio against the real server: each screen is rendered into a
// document, and what it shows is what the server holds.
//
// The door: making the first account, signing in, and who is let in.

import { afterAll, afterEach, describe, expect, test } from "bun:test";

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { PASSWORD, cookies, firstPerson } from "spool-testing";

import { standInForGoogle } from "../scripts/google-stand-in.ts";
import { SpoolClient } from "../src/api.ts";
import type { Fetch } from "../src/api.ts";
import { App } from "../src/app.tsx";
import {
  SEEDING,
  STOPPING,
  follow,
  heading,
  here,
  memory,
  open,
  parts,
  recording,
  rowsOf,
  screens,
  start,
  stopAll,
  wrapped,
} from "./harness.tsx";
import { companies } from "./seed.ts";

afterEach(() => {
  cleanup();
  window.history.replaceState(null, "", "/");
});

afterAll(stopAll, STOPPING);

describe("the door", () => {
  type Within = Pick<typeof screen, "getByLabelText" | "getByRole" | "getByText">;
  const fill = (label: string, value: string, where: Within = screen) =>
    fireEvent.change(where.getByLabelText(label), { target: { value } });
  const press = (name: string, where: Within = screen) => fireEvent.click(where.getByRole("button", { name }));

  test(
    "the first person makes their account from the server's link, and signs in with it after",
    async () => {
      const server = await start(companies, 11, { people: false });
      const browser = cookies();
      render(<App url={server.url} fetch={browser} storage={memory()} />);
      // Without the link there is nothing to do.
      await waitFor(() => expect(document.querySelector("[data-part=notice]")).not.toBeNull());
      expect(screen.queryByRole("button")).toBeNull();
      expect(document.title).toBe("Spool studio");
      cleanup();

      window.history.replaceState(null, "", `/setup#${server.setupCode}`);
      render(<App url={server.url} fetch={browser} storage={memory()} />);
      await screen.findByLabelText("Name");
      fill("Name", "Ada");
      fill("Username", "ada");
      fill("Password", PASSWORD);
      press("Create admin account");
      await heading("Overview");
      expect(here()).toBe("/");
      expect(parts("me")).toEqual(["Ada"]);

      press("Sign out");
      await screen.findByLabelText("Username");
      fill("Username", "ada");
      fill("Password", "not the password");
      press("Sign in");
      await screen.findByRole("alert");
      fill("Password", PASSWORD);
      press("Sign in");
      await heading("Overview");

      // A session that ends while the studio is open leads back to the door.
      await new SpoolClient({ url: server.url, fetch: browser }).signOut();
      follow(screens().getByRole("link", { name: "Log" }));
      await screen.findByLabelText("Username");
    },
    SEEDING,
  );

  test(
    "an admin adds a person and turns their account off",
    async () => {
      const server = await start(companies, 12);
      open(server, "/people");
      await heading("People");
      const table = await screen.findByRole("table");
      expect(rowsOf(table).map((cells) => cells.slice(0, 4))).toEqual([["Ada", "ada", "Admin", "On"]]);

      press("Add person");
      const adding = within(await screen.findByRole("dialog"));
      fill("Name", "Grace", adding);
      fill("Username", "grace", adding);
      fill("Password", PASSWORD, adding);
      press("Add person", adding);
      await screen.findByRole("status");
      await waitFor(() => expect(rowsOf(table).map((cells) => cells[1])).toEqual(["ada", "grace"]));
      const grace = new SpoolClient({ url: server.url, fetch: cookies() });
      expect((await grace.signIn("grace", PASSWORD)).person).toMatchObject({ name: "Grace", admin: false });

      fireEvent.click(within(screen.getAllByRole("row")[2]!).getByRole("button", { name: "Edit" }));
      const editing = within(await screen.findByRole("dialog"));
      // The words are pressed, as a label is.
      fireEvent.click(editing.getByText("Can sign in"));
      press("Save", editing);
      await waitFor(() => expect(rowsOf(table)[1]?.slice(0, 4)).toEqual(["Grace", "grace", "-", "Off"]));
      expect(await grace.session()).toMatchObject({ person: null });
    },
    SEEDING,
  );

  test(
    "an admin makes a token, reads it once, and revokes it",
    async () => {
      const server = await start(companies, 14);
      open(server, "/tokens");
      await heading("Tokens");
      const table = await screen.findByRole("table", { name: "Tokens" });
      // The token the tests call with was made on the server's machine.
      expect(rowsOf(table).map((cells) => cells.slice(0, 2))).toEqual([["api", "agent:api"]]);
      expect(rowsOf(table)[0]?.[2]).toMatch(/ by spool token add$/);

      press("Add token");
      const adding = within(await screen.findByRole("dialog"));
      fill("Name", "load", adding);
      press("Add token", adding);
      const said = await screen.findByRole("status");
      const secret = said.querySelector("[data-part=secret] code")?.textContent ?? "";
      expect(secret).toMatch(/^spool_[0-9a-f]{64}$/);
      await waitFor(() => expect(rowsOf(table).map((cells) => cells[0])).toEqual(["api", "load"]));
      expect(rowsOf(table)[1]?.slice(1, 4)).toEqual(["agent:load", expect.stringMatching(/ by Ada$/), "Never"]);

      // What presents it is let in. The list does not show it.
      const agent = new SpoolClient({ url: server.url, token: secret });
      expect((await agent.info()).api).toBe("v1");
      expect(table.textContent).not.toContain(secret);

      fireEvent.click(within(within(table).getAllByRole("row")[2]!).getByRole("button", { name: "Revoke" }));
      const revoking = within(await screen.findByRole("dialog"));
      press("Revoke", revoking);
      await waitFor(() => expect(rowsOf(table).map((cells) => cells[0])).toEqual(["api"]));
      expect(screen.getByRole("status").textContent).toBe("Revoked load.");
      expect(document.body.textContent).not.toContain(secret);
      expect(await agent.info().catch((error: unknown) => (error as { code: string }).code)).toBe("unauthorized");
    },
    SEEDING,
  );

  test(
    "an assistant is approved by whoever signs in at the address it sent them to, and an admin revokes it",
    async () => {
      const server = await start(companies, 16, { people: false });
      const ada = await firstPerson(server);
      const callback = "https://claude.ai/api/mcp/auth_callback";
      const registered = await fetch(`${server.url}/oauth/register`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ client_name: "Claude", redirect_uris: [callback] }),
      });
      const { client_id } = (await registered.json()) as { client_id: string };
      const request = new URLSearchParams({
        response_type: "code",
        client_id,
        redirect_uri: callback,
        code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        code_challenge_method: "S256",
        state: "kept",
      });

      // Not signed in, the door asks first, at the same address.
      const browser = cookies();
      const left: string[] = [];
      window.history.replaceState(null, "", `/oauth/authorize?${request}`);
      render(<App url={server.url} fetch={browser} storage={memory()} leave={(to) => left.push(to)} />);
      await screen.findByLabelText("Username");
      fill("Username", "ada");
      fill("Password", PASSWORD);
      press("Sign in");
      expect((await screen.findByText(/asks to read and change/)).textContent).toContain("as you, Ada");
      expect(document.querySelector("[data-part=host]")?.textContent).toBe("claude.ai");
      press("Approve");
      await waitFor(() => expect(left).toHaveLength(1));
      const back = new URL(left[0]!);
      expect(back.origin + back.pathname).toBe(callback);
      expect(back.searchParams.get("state")).toBe("kept");
      expect(back.searchParams.get("iss")).toBe(server.url);
      expect(back.searchParams.get("code")).toMatch(/^[0-9a-f]{64}$/);
      const token = await fetch(`${server.url}/oauth/token`, {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          grant_type: "authorization_code",
          code: back.searchParams.get("code")!,
          client_id,
          redirect_uri: callback,
          code_verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        }),
      });
      expect(token.status).toBe(200);
      cleanup();

      // The admin sees who let it in, and takes it back.
      window.history.replaceState(null, "", "/tokens");
      render(<App url={server.url} fetch={ada.fetch} storage={memory()} />);
      await heading("Tokens");
      const table = await screen.findByRole("table", { name: "Assistants" });
      await waitFor(() => expect(rowsOf(table).map((cells) => cells.slice(0, 2))).toEqual([["Claude", "claude.ai"]]));
      expect(rowsOf(table)[0]?.[2]).toMatch(/ by Ada$/);
      fireEvent.click(within(table).getByRole("button", { name: "Revoke" }));
      press("Revoke", within(await screen.findByRole("dialog")));
      await screen.findByText("No assistant has been approved.");
      expect(screen.getByRole("status").textContent).toBe("Revoked Claude.");
    },
    SEEDING,
  );

  test(
    "someone an admin added by email signs in with Google, and has no password to change",
    async () => {
      const google = await standInForGoogle();
      const server = await start(companies, 15, { environment: google.environment });
      // A browser that keeps what it is given and follows nothing by itself.
      const browser = () => {
        const kept = new Map<string, string>();
        const call: Fetch = async (input, init) => {
          const headers = new Headers(init.headers);
          if (kept.size > 0) headers.set("Cookie", [...kept].map(([name, value]) => `${name}=${value}`).join("; "));
          const response = await fetch(input, { ...init, headers, redirect: "manual" });
          for (const set of response.headers.getSetCookie()) {
            const [name = "", value = ""] = set.split(";")[0]!.split("=", 2);
            if (value === "") kept.delete(name);
            else kept.set(name, value);
          }
          return response;
        };
        return call;
      };
      /** Presses "Sign in with Google", signs in at the stand-in as `email`, and returns where that leads. */
      const throughGoogle = async (call: Fetch, link: string, email: string, personal = false) => {
        const leaving = await call(new URL(link), { method: "GET" });
        const at = new URL(leaving.headers.get("location")!);
        expect(at.origin + at.pathname).toBe(`${google.issuer}/authorize`);
        at.pathname = "/approve";
        at.searchParams.set("email", email);
        if (personal) at.searchParams.set("personal", "on");
        const approved = await fetch(at, { redirect: "manual" });
        const back = await call(new URL(approved.headers.get("location")!), { method: "GET" });
        expect(back.status).toBe(303);
        return back.headers.get("location");
      };

      // The door offers it, and comes back to the page it was opened at.
      window.history.replaceState(null, "", "/people?page=1");
      const hers = browser();
      render(<App url={server.url} fetch={hers} storage={memory()} />);
      const link = (await screen.findByText("Sign in with Google")).closest("a")!.href;
      expect(link).toBe(`${server.url}/auth/google?next=${encodeURIComponent("/people?page=1")}`);
      // Nobody added her yet, and the door says so when Google sends her back.
      const refused = await throughGoogle(hers, link, "grace@example.com");
      expect(refused).toBe("/people?page=1&google=unknown");
      cleanup();
      window.history.replaceState(null, "", refused!);
      render(<App url={server.url} fetch={hers} storage={memory()} />);
      expect((await screen.findByRole("alert")).textContent).toMatch(
        /^No account here has that Google account's email/,
      );
      // What the server added to the address is not kept in it.
      expect(here()).toBe("/people?page=1");
      expect((await screen.findByText("Sign in with Google")).closest("a")!.href).toBe(link);
      cleanup();

      // Ada adds her, by her email and with no password.
      open(server, "/people");
      await heading("People");
      const table = await screen.findByRole("table");
      expect(rowsOf(table).map((cells) => cells.slice(0, 6))).toEqual([["Ada", "ada", "-", "Password", "Admin", "On"]]);
      press("Add person");
      const adding = within(await screen.findByRole("dialog"));
      fill("Name", "Grace", adding);
      fill("Username", "grace", adding);
      press("Add person", adding);
      expect((await adding.findByRole("alert")).textContent).toBe("Give an email, a password, or both.");
      fill("Email", "Grace@example.com", adding);
      press("Add person", adding);
      await screen.findByRole("status");
      await waitFor(() =>
        expect(rowsOf(table)[1]?.slice(0, 6)).toEqual(["Grace", "grace", "grace@example.com", "Google", "-", "On"]),
      );
      cleanup();

      // A Google account that someone made with her address is not hers for that.
      const theirs = browser();
      const unvouched = await throughGoogle(theirs, link, "grace@example.com", true);
      expect(unvouched).toBe("/people?page=1&google=unvouched");
      window.history.replaceState(null, "", unvouched!);
      render(<App url={server.url} fetch={theirs} storage={memory()} />);
      expect((await screen.findByRole("alert")).textContent).toMatch(/is not Google's to answer for/);
      cleanup();

      // Now Google's word is enough, and she is at the page she opened.
      expect(await throughGoogle(hers, link, "grace@example.com")).toBe("/people?page=1");
      window.history.replaceState(null, "", "/people?page=1");
      render(<App url={server.url} fetch={hers} storage={memory()} />);
      await heading("People");
      expect(parts("me")).toEqual(["Grace"]);
      expect(screen.getByRole("button", { name: "Sign out" })).toBeTruthy();
      expect(screen.queryByRole("button", { name: "Change password" })).toBeNull();
      await google.stop();
    },
    SEEDING,
  );

  test(
    "says so when the server does not answer, and asks again",
    async () => {
      const server = await start(companies, 13);
      let down = true;
      const flaky = wrapped((input, init) =>
        down ? Promise.reject(new TypeError("connection refused")) : recording(input, init),
      );
      render(<App url={server.url} fetch={flaky} storage={memory()} />);
      expect((await screen.findByRole("alert")).textContent).toBe("The server did not answer: connection refused");
      down = false;
      fireEvent.click(screen.getByRole("button", { name: "Try again" }));
      await heading("Overview");
    },
    SEEDING,
  );
});
