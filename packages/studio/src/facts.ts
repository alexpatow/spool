// What a set of records says, field by field, and which of them say it.
// Nothing here decides which value is right: where records disagree, every
// value is listed with the records that hold it. A record that holds a field
// empty, or does not hold it, says nothing about it.

import type { ObservationRecord } from "./api.ts";
import { plain } from "./format.ts";

export interface Said {
  /** The value as text. */
  value: string;
  records: ObservationRecord[];
}

export interface Fact {
  field: string;
  /** Each value some record holds, the one most records hold first. There is at least one. */
  values: Said[];
  /** Whether every record that holds a value holds the same one. */
  agreed: boolean;
}

/** The value of one field of a record as text, or `null` when it holds none. */
export function valueOf(record: Pick<ObservationRecord, "payload">, field: string): string | null {
  const payload = record.payload;
  if (typeof payload !== "object" || payload === null || Array.isArray(payload)) return null;
  const value = (payload as Record<string, unknown>)[field];
  if (value === null || value === undefined) return null;
  const text = plain(value).trim();
  return text === "" ? null : text;
}

/** The fields the records hold, in the order a reader first meets them. */
export function fieldsOf(records: Array<Pick<ObservationRecord, "payload">>): string[] {
  const fields: string[] = [];
  for (const record of records) {
    const payload = record.payload;
    if (typeof payload !== "object" || payload === null || Array.isArray(payload)) continue;
    for (const field of Object.keys(payload)) if (!fields.includes(field)) fields.push(field);
  }
  return fields;
}

/** Whether two values are the same to a reader: case and the space around them aside. */
export function same(a: string | null, b: string | null): boolean {
  return a !== null && b !== null && a.trim().toLowerCase() === b.trim().toLowerCase();
}

/**
 * The versions of rows that nothing has replaced. An earlier version says
 * what a row used to say, which is not what the records say now.
 */
export function latest(records: ObservationRecord[]): ObservationRecord[] {
  return records.filter((record) => !record.superseded_by);
}

/** The fields some record holds a value for, with every value held. */
export function facts(records: ObservationRecord[]): Fact[] {
  const found: Fact[] = [];
  for (const field of fieldsOf(records)) {
    const values: Said[] = [];
    for (const record of records) {
      const value = valueOf(record, field);
      if (value === null) continue;
      const said = values.find((other) => other.value === value);
      if (said) said.records.push(record);
      else values.push({ value, records: [record] });
    }
    if (values.length === 0) continue;
    // The order they were met in stands among values as many records hold.
    values.sort((a, b) => b.records.length - a.records.length);
    found.push({ field, values, agreed: values.length === 1 });
  }
  return found;
}

/** How many of the records came from each source, the source of most first. */
export function sourcesOf(
  records: Array<Pick<ObservationRecord, "source">>,
): Array<{ source: string; observations: number }> {
  const counts = new Map<string, number>();
  for (const record of records) counts.set(record.source, (counts.get(record.source) ?? 0) + 1);
  return [...counts]
    .map(([source, observations]) => ({ source, observations }))
    .sort((a, b) => b.observations - a.observations || a.source.localeCompare(b.source));
}
