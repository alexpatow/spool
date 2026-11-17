import { describe, expect, test } from "bun:test";

import type { ObservationRecord, Op } from "../src/api.ts";
import {
  count,
  describeOp,
  excerpt,
  mentionOf,
  observationName,
  observationsOf,
  plain,
  segments,
  short,
  when,
} from "../src/format.ts";
import { isReference } from "../src/find.ts";
import { href, parseHash } from "../src/router.ts";

const A = "2060cadc-cebd-9817-f61a-f4fd7fd74ee7";
const B = "42a8e89a-91a6-6c9c-2295-010d61b8803a";
const X = "01a1e95d-fb10-7069-8e60-0a0cc1a955db";
const Y = "01a1e95d-fb10-7069-8e60-0a0cc1a9e882";

function observation(fields: Partial<ObservationRecord>): ObservationRecord {
  return {
    id: A,
    entity_type: "company",
    source: "crm",
    key: "c-1",
    payload: {},
    ingested_at: "2026-11-17T11:53:18Z",
    ingest_run: 1,
    ...fields,
  } as ObservationRecord;
}

describe("what is shown", () => {
  test("a number of things is said in the singular and the plural", () => {
    expect(count(0, "identity")).toBe("0 identities");
    expect(count(1, "identity")).toBe("1 identity");
    expect(count(1, "run")).toBe("1 run");
    expect(count(2, "run")).toBe("2 runs");
  });

  test("a time is shown to the second, where the reader is", () => {
    expect(when("2026-11-17T11:53:18.123456Z", "Europe/Stockholm")).toBe("2026-11-17 12:53:18");
    expect(when("2026-06-30T23:59:59Z", "UTC")).toBe("2026-06-30 23:59:59");
    expect(when("not a time")).toBe("not a time");
  });

  test("a short ID keeps the end, where IDs minted together differ", () => {
    expect(short(X)).toBe("01a1e95d…55db");
    expect(short(Y)).toBe("01a1e95d…e882");
    expect(short("crm:c-1")).toBe("crm:c-1");
  });

  test("an observation is named by its source and key, and a mention by its document and span", () => {
    expect(observationName(observation({}))).toBe("crm:c-1");
    const mention = observation({
      source: "news",
      key: `${X}#company:4-13`,
      evidence: {
        document: X,
        key: "01.txt",
        start: 4,
        end: 13,
        text: "Acme Ltd",
        extractor: "suffix",
        version: "abc",
      },
    });
    expect(mentionOf(mention)).toMatchObject({ document: X, key: "01.txt", start: 4, end: 13 });
    expect(observationName(mention)).toBe("news:01.txt[4-13]");
    expect(mentionOf(observation({ evidence: { note: "typed in" } }))).toBeNull();
  });

  test("a value of a payload is shown as it would be written", () => {
    expect(plain("Cork")).toBe("Cork");
    expect(plain(null)).toBe("-");
    expect(plain(12)).toBe("12");
    expect(plain({ a: [1, 2] })).toBe('{"a":[1,2]}');
  });

  test("every kind of operation is put in words, with the things it names", () => {
    const ops: Op[] = [
      { op: "create_identity", identity: X, entity_type: "company" },
      { op: "assign", observation: A, to: X },
      { op: "assign", observation: A, from: X, to: Y },
      { op: "assign", observation: A, from: Y },
      { op: "merge", from: X, into: Y },
      { op: "reinstate", identity: X },
      { op: "constrain", kind: "must_link", a: A, b: B },
      { op: "unconstrain", kind: "cannot_link", a: A, b: B },
    ];
    const said = ops.map((op) =>
      describeOp(op)
        .map((part) => (typeof part === "string" ? part : `<${part.kind} ${short(part.id)}>`))
        .join(""),
    );
    expect(said).toEqual([
      "create company identity <identity 01a1e95d…55db>",
      "place observation <observation 2060cadc…4ee7> in <identity 01a1e95d…55db>",
      "move observation <observation 2060cadc…4ee7> from <identity 01a1e95d…55db> to <identity 01a1e95d…e882>",
      "take observation <observation 2060cadc…4ee7> out of <identity 01a1e95d…e882>",
      "merge <identity 01a1e95d…55db> into <identity 01a1e95d…e882>",
      "reinstate identity <identity 01a1e95d…55db>",
      "constrain <observation 2060cadc…4ee7> and <observation 42a8e89a…803a>: must_link",
      "remove the cannot_link constraint on <observation 2060cadc…4ee7> and <observation 42a8e89a…803a>",
    ]);
    expect(observationsOf(ops)).toEqual([A, B]);
  });
});

describe("the text of a document", () => {
  test("is cut where mentions begin and end, by characters and not by UTF-16 units", () => {
    const text = "𝒜cme Ltd bought Ørsted A/S.";
    const cut = segments(text, [
      { start: 0, end: 8, value: "one" },
      { start: 16, end: 26, value: "two" },
    ]);
    expect(cut.map((segment) => [segment.start, segment.text, segment.covered])).toEqual([
      [0, "𝒜cme Ltd", ["one"]],
      [8, " bought ", []],
      [16, "Ørsted A/S", ["two"]],
      [26, ".", []],
    ]);
  });

  test("keeps every mention where two overlap", () => {
    const cut = segments("Bank of Ireland Group", [
      { start: 0, end: 15, value: "bank" },
      { start: 8, end: 21, value: "group" },
    ]);
    expect(cut.map((segment) => [segment.text, segment.covered])).toEqual([
      ["Bank of ", ["bank"]],
      ["Ireland", ["bank", "group"]],
      [" Group", ["group"]],
    ]);
  });

  test("leaves out a span that is not in the text", () => {
    const cut = segments("short", [{ start: 3, end: 40, value: "long" }]);
    expect(cut).toEqual([{ text: "short", start: 0, covered: [] }]);
  });

  test("is excerpted around a mention, on one line", () => {
    const text = `${"a".repeat(100)}\nAcme Ltd\n${"b".repeat(100)}`;
    const cut = excerpt(text, 101, 109, 10);
    expect(cut).toEqual({
      before: `${"a".repeat(9)} `,
      text: "Acme Ltd",
      after: ` ${"b".repeat(9)}`,
      cutBefore: true,
      cutAfter: true,
    });
    expect(excerpt("𝒜cme Ltd rose.", 0, 8)).toEqual({
      before: "",
      text: "𝒜cme Ltd",
      after: " rose.",
      cutBefore: false,
      cutAfter: false,
    });
    expect(excerpt("short", 3, 40)).toBeNull();
  });
});

describe("an address", () => {
  test("names a screen and what narrows it", () => {
    expect(parseHash("")).toEqual({ path: [], query: {} });
    expect(parseHash("#/")).toEqual({ path: [], query: {} });
    expect(parseHash("#/runs/9")).toEqual({ path: ["runs", "9"], query: {} });
    expect(parseHash("#/observations?source=crm&key=c%2F1&unassigned=true")).toEqual({
      path: ["observations"],
      query: { source: "crm", key: "c/1", unassigned: "true" },
    });
  });

  test("is written so that it reads back the same", () => {
    expect(href([])).toBe("#/");
    expect(href(["runs", 9])).toBe("#/runs/9");
    const address = href(["observations"], { source: "crm", key: "a b&c", unassigned: true, type: "", document: null });
    expect(address).toBe("#/observations?source=crm&key=a+b%26c&unassigned=true");
    expect(parseHash(address)).toEqual({
      path: ["observations"],
      query: { source: "crm", key: "a b&c", unassigned: "true" },
    });
    expect(parseHash(href(["documents", "a/b"]))).toEqual({ path: ["documents", "a/b"], query: {} });
  });
});

describe("a reference", () => {
  test("is an ID, or a source and whatever the source calls its row", () => {
    for (const text of [A, "crm:c-104", "news:Annual Report.txt", "web:https://example.com/a b", "news:a.txt[0-14]"]) {
      expect([text, isReference(text)]).toEqual([text, true]);
    }
    for (const text of ["", "halden energy", "crm:", ":c-104", "halden energy: a note", A.slice(0, 20)]) {
      expect([text, isReference(text)]).toEqual([text, false]);
    }
  });
});
