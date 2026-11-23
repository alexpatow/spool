// The client against the real server: people, tokens, and calls it refuses.

import { afterAll, describe, expect, test } from "bun:test";

import { SpoolClient } from "../src/index.ts";
import { cleanUp, failureOf, thingsServer } from "./fixtures.ts";
import { PASSWORD, cookies, firstPerson } from "./server.ts";

afterAll(cleanUp);

describe("people and tokens", () => {
  test("the first person sets the server up, adds another, and each writes under their own id", async () => {
    const server = await thingsServer(21);
    const nobody = new SpoolClient({ url: server.url, fetch: cookies() });
    expect(await nobody.session()).toEqual({ person: null, setup: true, dev: false, google: false });
    expect((await failureOf(nobody.info())).code).toBe("unauthorized");

    const ada = await firstPerson(server);
    expect(ada.person).toMatchObject({ username: "ada", admin: true, active: true });
    expect(await ada.client.session()).toEqual({ person: ada.person, setup: false, dev: false, google: false });

    const grace = await ada.client.addPerson({ username: "grace", name: "Grace", password: PASSWORD });
    expect(grace.admin).toBe(false);
    expect((await ada.client.people()).map((person) => person.username)).toEqual(["ada", "grace"]);

    const hers = new SpoolClient({ url: server.url, fetch: cookies() });
    expect((await failureOf(hers.signIn("grace", "not her password"))).code).toBe("unauthorized");
    expect((await hers.signIn("grace", PASSWORD)).person).toEqual(grace);
    expect((await failureOf(hers.addPerson({ username: "x", name: "X", password: PASSWORD }))).code).toBe("forbidden");
    expect((await failureOf(server.client.changePerson(grace.id, { admin: true }))).code).toBe("forbidden");

    const [identity] = (await hers.mintIds({ identities: 1 })).identities;
    const made = await hers.commit({ ops: [{ op: "create_identity", identity: identity!, entity_type: "thing" }] });
    const { changesets } = await hers.changesets({ after: made.seq - 1 });
    expect(changesets[0]!.changeset.actor).toBe(`user:${grace.id}`);

    // An account that is turned off is signed out, and cannot sign in.
    await ada.client.changePerson(grace.id, { active: false });
    expect((await failureOf(hers.info())).code).toBe("unauthorized");
    expect((await failureOf(hers.signIn("grace", PASSWORD))).code).toBe("unauthorized");

    await ada.client.signOut();
    expect((await ada.client.session()).person).toBeNull();
  });

  test("where the server signs people in with Google, a person is added by their email", async () => {
    // Nothing answers at the issuer: the server starts without it.
    const server = await thingsServer(23, {
      SPOOL_GOOGLE_CLIENT_ID: "a-client",
      SPOOL_GOOGLE_CLIENT_SECRET: "its-secret",
      SPOOL_GOOGLE_ISSUER: "http://127.0.0.1:9",
    });
    const ada = await firstPerson(server);
    expect(await ada.client.session()).toMatchObject({ google: true });
    expect(ada.person).toMatchObject({ email: null, password: true });

    const grace = await ada.client.addPerson({ username: "grace", name: "Grace", email: "Grace@Example.com" });
    expect(grace).toMatchObject({ email: "grace@example.com", password: false });
    expect((await failureOf(ada.client.addPerson({ username: "bob", name: "Bob" }))).code).toBe("invalid");
    expect((await ada.client.changePerson(ada.person.id, { email: "ada@example.com" })).email).toBe("ada@example.com");
    expect((await ada.client.changePerson(ada.person.id, { email: "" })).email).toBeNull();

    // A browser opens the address, and is sent back with the reason when Google lets nobody in.
    const address = ada.client.googleSignIn("/review?status=open");
    expect(address).toBe(`${server.url}/auth/google?next=%2Freview%3Fstatus%3Dopen`);
    const sent = await fetch(address, { redirect: "manual" });
    expect([sent.status, sent.headers.get("location")]).toEqual([303, "/review?status=open&google=unreachable"]);
  });

  test("an admin makes a token, which writes under its own name until it is revoked", async () => {
    const server = await thingsServer(22);
    const ada = await firstPerson(server);
    const { token, secret } = await ada.client.addToken("load");
    expect(token).toMatchObject({ name: "load", created_by: ada.person.id, last_used_at: null });
    expect((await failureOf(ada.client.addToken("load"))).code).toBe("conflict");

    const agent = new SpoolClient({ url: server.url, token: secret });
    const [identity] = (await agent.mintIds({ identities: 1 })).identities;
    const made = await agent.commit({ ops: [{ op: "create_identity", identity: identity!, entity_type: "thing" }] });
    const { changesets } = await agent.changesets({ after: made.seq - 1 });
    expect(changesets[0]!.changeset.actor).toBe("agent:load");

    // A token manages neither tokens nor people.
    expect((await failureOf(agent.tokens())).code).toBe("forbidden");
    expect((await failureOf(agent.addToken("another"))).code).toBe("forbidden");

    // The one the tests call with was made on the server's machine.
    const listed = await ada.client.tokens();
    expect(listed.map((each) => [each.name, each.created_by])).toEqual([
      ["api", null],
      ["load", ada.person.id],
    ]);
    expect(listed[1]!.last_used_at).not.toBeNull();

    expect((await ada.client.revokeToken(token.id)).name).toBe("load");
    expect((await failureOf(agent.info())).code).toBe("unauthorized");
    expect((await failureOf(ada.client.revokeToken(token.id))).code).toBe("not_found");
    expect((await server.client.info()).api).toBe("v1");
  });
});

describe("failures", () => {
  test("a wrong token is refused", async () => {
    const { url, client, token } = await thingsServer(7);
    const stranger = new SpoolClient({ url, token: `${token}-but-wrong` });
    const failure = await failureOf(stranger.info());
    expect(failure.status).toBe(401);
    expect(failure.code).toBe("unauthorized");
    expect(await stranger.healthy()).toBe(true);
    expect((await client.info()).store.head).toBe(0);
    // A trailing slash on the URL is not part of the path.
    expect((await new SpoolClient({ url: `${url}/`, token }).info()).api).toBe("v1");
  });

  test("what the server will not take is described", async () => {
    const { client } = await thingsServer(8);
    const undeclared = await failureOf(client.match({ entity_type: "gadget" }));
    expect(undeclared).toMatchObject({ status: 400, code: "invalid" });
    expect(undeclared.message).toContain("`gadget` is not declared");
    expect((await failureOf(client.run(999))).code).toBe("not_found");

    const minted = await client.mintIds({ identities: 1, changesets: 1 });
    const rejected = await failureOf(
      client.commit({
        id: minted.changesets[0]!,
        base: 0,
        ops: [
          { op: "create_identity", identity: minted.identities[0]!, entity_type: "thing" },
          { op: "assign", record: "00000000-0000-7000-8000-000000000002", to: minted.identities[0]! },
        ],
      }),
    );
    expect(rejected.code).toBe("rejected");
    expect(rejected.rejections).toHaveLength(1);
    expect(rejected.rejections[0]).toMatchObject({ op_index: 1, reason: { reason: "unknown_record" } });
  });

  test("no server at all is not healthy", async () => {
    const nowhere = new SpoolClient({ url: "http://127.0.0.1:1", token: "of-no-server" });
    expect(await nowhere.healthy()).toBe(false);
  });
});
