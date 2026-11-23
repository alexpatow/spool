// The client against the real server: what the project's own code runs, documents,
// and runs that take a while.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";

import type { RecordEntry } from "../src/index.ts";
import {
  cleanUp,
  failureOf,
  loadThings,
  removeLater,
  startServer,
  thingsServer,
  twoAnvilsAndAHammer,
} from "./fixtures.ts";
import { hasUv, temporaryDirectory, writeExtractor, writeStrategy, writeThings } from "./server.ts";
import type { Server } from "./server.ts";

afterAll(cleanUp);

// These start `uv`, as the server does for a strategy written in Python.
describe.skipIf(!hasUv)("a strategy of the project", () => {
  const everyPair = `
from spool import strategy


@strategy
def judge(features):
    ids = sorted(features.column("record_id").to_pylist())
    new = {
        row["record_id"]
        for row in features.select(["record_id", "is_new"]).to_pylist()
        if row["is_new"]
    }
    return [
        {"left_id": left, "right_id": right, "verdict": "match", "score": 0.9, "detail": {"why": "all one"}}
        for index, left in enumerate(ids)
        for right in ids[index + 1 :]
        if left in new or right in new
    ]
`;
  const failing = `
from spool import strategy


@strategy
def judge(candidates):
    raise RuntimeError("no model")
`;
  let server: Server;

  beforeAll(async () => {
    const project = removeLater(temporaryDirectory());
    writeThings(project);
    writeStrategy(project, "all_one.py", everyPair);
    writeStrategy(project, "failing.py", failing);
    server = await startServer(project, 5);
  });

  test("is listed, runs on the server, and a rematch only suggests", async () => {
    const { client } = server;
    const [thing] = (await client.info()).store.entity_types;
    expect(thing?.strategies?.map((strategy) => [strategy.name, strategy.runtime])).toEqual([
      ["all_one", "python"],
      ["failing", "python"],
    ]);
    const allOne = thing!.strategies![0]!;

    await loadThings(client, twoAnvilsAndAHammer);
    const rules = await client.match({ entity_type: "thing" });
    expect(rules?.status).toBe("succeeded");
    expect(rules?.resolution?.suggestions ?? null).toBeNull();
    const before = await client.identities({ entity_type: "thing" });
    expect(before.identities).toHaveLength(2);

    // The strategy would make one thing of everything. A rematch says so
    // and leaves the two identities as they are.
    const head = (await client.info()).store.head;
    const rematch = await client.match({ entity_type: "thing", strategy: { name: "all_one" }, rematch: true });
    expect(rematch?.status).toBe("succeeded");
    expect(rematch?.strategy).toEqual({ name: "all_one", version: allOne.hash });
    const suggestions = rematch?.resolution?.suggestions;
    expect(suggestions?.splits).toEqual([]);
    expect(suggestions?.merges).toHaveLength(1);
    const [merge] = suggestions!.merges;
    expect([...merge!.identities].sort()).toEqual(before.identities.map((identity) => identity.id).sort());
    expect(merge!.matches).toBe(2);
    expect((await client.info()).store.head).toBe(head);

    // Without `rematch` it places what is new.
    await loadThings(client, [{ id: "4", code: "saw" }]);
    const run = await client.match({ entity_type: "thing", strategy: { name: "all_one" } });
    expect(run?.status).toBe("succeeded");
    expect(run?.resolution?.identities_created).toBe(0);
  });

  test("that fails is a failed run with what it said", async () => {
    const { client } = server;
    await loadThings(client, [{ id: "5", code: "file" }]);
    const failure = await failureOf(client.match({ entity_type: "thing", strategy: { name: "failing" } }));
    expect(failure.status).toBe(500);
    expect(failure.code).toBe("strategy_failed");
    expect(failure.message).toContain("no model");
    const [failed] = await client.runs({ status: "failed" });
    expect(failed?.strategy?.name).toBe("failing");
    expect(failed?.error).toContain("no model");
  });
});

describe("documents", () => {
  test("are stored once, listed without their text, and read with it", async () => {
    const { client } = await thingsServer(21);
    const run = await client.addDocuments("notes", [
      { key: "a", text: "🔨 Anvil and Hammer.", uri: "https://notes.test/a" },
      { key: "b", text: "Hammer again." },
      { key: "a", text: "🔨 Anvil and Hammer.", uri: "https://notes.test/a" },
    ]);
    expect(run.kind).toBe("load");
    expect(run.status).toBe("succeeded");
    expect(run.load).toMatchObject({ received: 3, inserted: 2, duplicates: 1 });
    expect((await client.info()).store.documents).toBe(2);

    const { documents, next } = await client.documents({ source: "notes" });
    expect(next ?? null).toBeNull();
    expect(documents.map((document) => document.key).sort()).toEqual(["a", "b"]);
    const a = documents.find((document) => document.key === "a")!;
    // Lengths count code points: the hammer is one.
    expect(a.length).toBe(19);
    expect(a.text ?? null).toBeNull();
    expect(a.uri).toBe("https://notes.test/a");
    expect((await client.document(a.id)).text).toBe("🔨 Anvil and Hammer.");

    // A changed text is the next version of the document.
    const changed = await client.addDocuments("notes", [{ key: "a", text: "Only an Anvil." }]);
    expect(changed.load?.superseded).toBe(1);
    const [latest] = (await client.documents({ key: "a", latest: true })).documents;
    expect(latest?.supersedes).toBe(a.id);
    expect((await client.document(a.id)).superseded_by).toBe(latest!.id);

    const first = await client.documents({ limit: 2 });
    expect(first.documents).toHaveLength(2);
    const second = await client.documents({ limit: 2, after: first.next! });
    expect(second.documents).toHaveLength(1);

    const missing = await failureOf(client.document("00000000-0000-0000-0000-000000000000"));
    expect(missing.code).toBe("not_found");
    const unknown = await failureOf(client.extract("words"));
    expect(unknown.status).toBe(400);
    expect(unknown.message).toContain("the project has no extractor `words`");
  });
});

// These start `uv`, as the server does for an extractor written in Python.
describe.skipIf(!hasUv)("an extractor of the project", () => {
  const words = `
import re

from spool import extractor


@extractor
def find(document):
    for found in re.finditer(r"[A-Z][a-z]+", document.text):
        yield document.mention("thing", found.start(), found.end(), code=found.group().lower())
`;
  const failing = `
from spool import extractor


@extractor
def find(documents):
    raise RuntimeError("no model")
`;
  let server: Server;

  beforeAll(async () => {
    const project = removeLater(temporaryDirectory());
    writeThings(project);
    writeExtractor(project, "words.py", words);
    writeExtractor(project, "failing.py", failing);
    server = await startServer(project, 6);
  });

  test("is listed, reads documents, and what it finds is resolved", async () => {
    const { client } = server;
    const extractors = (await client.info()).store.extractors ?? [];
    expect(extractors.map((extractor) => [extractor.name, extractor.runtime])).toEqual([
      ["failing", "python"],
      ["words", "python"],
    ]);

    await client.addDocuments("notes", [
      { key: "a", text: "🔨 Anvil and Hammer." },
      { key: "b", text: "Hammer again." },
    ]);
    const run = await client.extract("words");
    expect(run.kind).toBe("extract");
    expect(run.status).toBe("succeeded");
    expect(run.extractor).toEqual({ name: "words", version: extractors[1]!.hash });
    expect(run.extraction).toMatchObject({ documents: 2, mentions: 3, inserted: 3 });

    // A mention is a record that says where it was found.
    const a = (await client.documents({ key: "a" })).documents[0]!;
    expect(a.extractions).toEqual([{ extractor: "words", version: extractors[1]!.hash, run: run.run, mentions: 2 }]);
    const { records } = await client.records({ document: a.id });
    const code = (record: RecordEntry) => (record.payload as { code: string }).code;
    expect(records.map(code).sort()).toEqual(["anvil", "hammer"]);
    const anvil = records.find((record) => code(record) === "anvil")!;
    expect(anvil.key).toBe(`${a.id}#thing:2-7`);
    expect(anvil.evidence).toMatchObject({ document: a.id, key: "a", start: 2, end: 7, text: "Anvil" });

    // Matching treats mentions as it treats rows: the two hammers are one thing.
    const resolved = await client.match({ entity_type: "thing" });
    expect(resolved?.resolution?.identities_created).toBe(2);
    const hammers = (await client.records({ entity_type: "thing" })).records.filter(
      (record) => code(record) === "hammer",
    );
    expect(hammers).toHaveLength(2);
    expect(hammers[0]!.identity).toBe(hammers[1]!.identity!);

    // What was read is not read again unless asked.
    expect((await client.extract("words")).extraction?.documents).toBe(0);
    const again = await client.extract("words", { again: true, source: "notes", limit: 1 });
    expect(again.extraction).toMatchObject({ documents: 1, inserted: 0 });
  });

  test("that fails is a failed run with what it said", async () => {
    const { client } = server;
    await client.addDocuments("notes", [{ key: "c", text: "Saw." }]);
    const failure = await failureOf(client.extract("failing"));
    expect(failure.status).toBe(500);
    expect(failure.code).toBe("extractor_failed");
    expect(failure.message).toContain("no model");
    const [failed] = await client.runs({ kind: "extract", status: "failed" });
    expect(failed?.extractor?.name).toBe("failing");
    expect(failed?.error).toContain("no model");
  });
});

describe("calls that take a while", () => {
  test("a run that outlasts the wait is followed until it ends", async () => {
    const { client } = await thingsServer(6);
    const rows = Array.from({ length: 30_000 }, (_, index) => ({ id: String(index), code: String(index) }));

    const started = await client.load({ source: "bulk", entity_type: "thing", rows, wait_ms: 0 });
    expect(started.status).toBe("running");
    const finished = await client.waitForRun(started.run);
    expect(finished.status).toBe("succeeded");
    expect(finished.load?.inserted).toBe(30_000);

    const resolving = await client.match({ entity_type: "thing", wait_ms: 0 });
    expect(resolving?.status).toBe("running");
    // A wait that is too short hands back the run as it is.
    expect((await client.waitForRun(resolving!.run, { timeoutMs: 0 })).status).toBe("running");
    const resolved = await client.waitForRun(resolving!.run, { timeoutMs: 120_000 });
    expect(resolved.status).toBe("succeeded");
    expect(resolved.resolution?.identities_created).toBe(30_000);
  });
});
