// What a set of records says, worked out from the records alone.

import { describe, expect, test } from "bun:test";

import type { ObservationRecord } from "../src/api.ts";
import { facts, fieldsOf, latest, same, sourcesOf, valueOf } from "../src/facts.ts";

let made = 0;

function record(
  source: string,
  key: string,
  payload: unknown,
  more: Partial<ObservationRecord> = {},
): ObservationRecord {
  made++;
  return {
    id: `00000000-0000-7000-8000-${String(made).padStart(12, "0")}`,
    entity_type: "company",
    source,
    key,
    payload,
    ingest_run: 1,
    ingested_at: "2026-01-01T00:00:00Z",
    ...more,
  } as ObservationRecord;
}

describe("what records say", () => {
  test("a value is read as it would be written, and an empty one is none", () => {
    const row = record("crm", "c-1", { name: " Acme ", staff: 12, open: true, tags: ["a", "b"], city: "", note: null });
    expect(valueOf(row, "name")).toBe("Acme");
    expect(valueOf(row, "staff")).toBe("12");
    expect(valueOf(row, "open")).toBe("true");
    expect(valueOf(row, "tags")).toBe('["a","b"]');
    expect(valueOf(row, "city")).toBeNull();
    expect(valueOf(row, "note")).toBeNull();
    expect(valueOf(row, "missing")).toBeNull();
    // A payload that is not an object has no fields.
    expect(valueOf(record("crm", "c-2", "Acme"), "name")).toBeNull();
    expect(valueOf(record("crm", "c-3", ["Acme"]), "0")).toBeNull();
  });

  test("the fields are listed in the order a reader first meets them", () => {
    const rows = [
      record("crm", "c-1", { name: "Acme", city: "Leeds" }),
      record("registry", "X1", { legal_name: "ACME LTD", city: "Leeds", number: "X1" }),
      record("crm", "c-2", "not an object"),
    ];
    expect(fieldsOf(rows)).toEqual(["name", "city", "legal_name", "number"]);
    expect(fieldsOf([])).toEqual([]);
  });

  test("each field lists every value held, with the records that hold it, the value most hold first", () => {
    const a = record("crm", "c-1", { name: "Acme", city: "Leeds", site: null });
    const b = record("crm", "c-2", { name: "Acme Ltd", city: "Leeds" });
    const c = record("registry", "X1", { name: "Acme Ltd", number: "X1", site: "" });
    const said = facts([a, b, c]);
    expect(said.map((fact) => fact.field)).toEqual(["name", "city", "number"]);

    const [name, city, number] = said;
    expect(name!.agreed).toBe(false);
    expect(name!.values.map((value) => [value.value, value.records.map((held) => held.key)])).toEqual([
      ["Acme Ltd", ["c-2", "X1"]],
      ["Acme", ["c-1"]],
    ]);
    // A record that does not hold a field says nothing about it, so it is not a disagreement.
    expect(city!.agreed).toBe(true);
    expect(city!.values.map((value) => [value.value, value.records.map((held) => held.key)])).toEqual([
      ["Leeds", ["c-1", "c-2"]],
    ]);
    expect(number!.values.map((value) => value.records.map((held) => held.source))).toEqual([["registry"]]);
    // A field no record holds a value for is left out.
    expect(said.some((fact) => fact.field === "site")).toBe(false);
  });

  test("values as many records hold stay in the order they were met", () => {
    const said = facts([
      record("crm", "c-1", { name: "One" }),
      record("crm", "c-2", { name: "Two" }),
      record("crm", "c-3", { name: "Three" }),
    ]);
    expect(said[0]!.values.map((value) => value.value)).toEqual(["One", "Two", "Three"]);
  });

  test("two values are the same to a reader whatever their case, and none is the same as nothing", () => {
    expect(same("Acme Ltd", " acme ltd ")).toBe(true);
    expect(same("Acme", "Acme Ltd")).toBe(false);
    expect(same(null, null)).toBe(false);
    expect(same("Acme", null)).toBe(false);
  });

  test("an earlier version of a row is not what the records say now", () => {
    const first = record("crm", "c-1", { name: "Acme" }, { superseded_by: "00000000-0000-7000-8000-999999999999" });
    const second = record("crm", "c-1", { name: "Acme Group" });
    expect(latest([first, second])).toEqual([second]);
    expect(facts(latest([first, second]))[0]!.agreed).toBe(true);
  });

  test("the sources are counted, the one that gave the most first", () => {
    const rows = [
      record("registry", "X1", {}),
      record("crm", "c-1", {}),
      record("crm", "c-2", {}),
      record("web", "w-1", {}),
    ];
    expect(sourcesOf(rows)).toEqual([
      { source: "crm", observations: 2 },
      { source: "registry", observations: 1 },
      { source: "web", observations: 1 },
    ]);
    expect(sourcesOf([])).toEqual([]);
  });
});
