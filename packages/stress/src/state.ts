// What a reader can learn over HTTP about who is who, and whether the
// answers of the different calls agree with each other.

import type { SpoolClient } from "spool-client";

export interface State {
  head: number;
  /** The identity of each record, by record ID. Null: it has none. */
  placed: Map<string, string | null>;
  /** How many members each identity is listed with. */
  identities: Map<string, number>;
}

const PAGE = 1000;

/** Reads every record and identity. Nothing else may write meanwhile. */
export async function read(client: SpoolClient): Promise<State> {
  const placed = new Map<string, string | null>();
  for (let after: string | undefined; ;) {
    const page = await client.records({ after, limit: PAGE });
    for (const record of page.records) placed.set(record.id, record.identity ?? null);
    if (page.next == null) break;
    after = page.next;
  }
  const identities = new Map<string, number>();
  for (let after: string | undefined; ;) {
    const page = await client.identities({ after, limit: PAGE });
    for (const identity of page.identities) identities.set(identity.id, identity.members);
    if (page.next == null) break;
    after = page.next;
  }
  return { head: (await client.info()).store.head, placed, identities };
}

/**
 * Who is with whom, without the identities' IDs: one line for each identity
 * that has members, and one for each record that has none. Two stores
 * that hold the same records agree on who is who when these are equal.
 */
export function groups(state: State): string[] {
  const members = new Map<string, string[]>();
  const lines: string[] = [];
  for (const [record, identity] of state.placed) {
    if (identity === null) lines.push(`alone ${record}`);
    else members.set(identity, [...(members.get(identity) ?? []), record]);
  }
  for (const group of members.values()) lines.push(group.sort().join(" "));
  return lines.sort();
}

/**
 * Where the calls disagree with each other, or with a rule the engine
 * keeps. Empty when they do not. At most `sample` identities are read one
 * by one.
 */
export async function faults(client: SpoolClient, state: State, sample = 40): Promise<string[]> {
  const found: string[] = [];
  const counted = new Map<string, number>();
  for (const [record, identity] of state.placed) {
    if (identity === null) continue;
    if (!state.identities.has(identity)) found.push(`record ${record} is in ${identity}, which is not listed`);
    counted.set(identity, (counted.get(identity) ?? 0) + 1);
  }
  for (const [identity, members] of state.identities) {
    const pointing = counted.get(identity) ?? 0;
    if (pointing !== members) {
      found.push(`identity ${identity} is listed with ${members} members, and ${pointing} records name it`);
    }
  }

  const info = await client.info();
  const totals = { records: 0, identities: 0, unassigned: 0 };
  for (const type of info.store.entity_types) {
    totals.records += type.records;
    totals.identities += type.identities;
    totals.unassigned += type.unassigned;
  }
  const unassigned = [...state.placed.values()].filter((identity) => identity === null).length;
  if (totals.records !== state.placed.size) {
    found.push(`info counts ${totals.records} records, and ${state.placed.size} are listed`);
  }
  // An identity whose records all left it is still listed, and not counted.
  const holding = [...state.identities.values()].filter((members) => members > 0).length;
  if (totals.identities !== holding) {
    found.push(`info counts ${totals.identities} identities, and ${holding} listed ones hold a record`);
  }
  if (totals.unassigned !== unassigned) {
    found.push(`info counts ${totals.unassigned} records without an identity, and ${unassigned} are listed`);
  }

  // The log has every position up to its head, once each.
  let position = 0;
  for (;;) {
    const page = await client.changesets({ after: position, limit: PAGE });
    for (const entry of page.changesets) {
      if (entry.seq !== position + 1) found.push(`the log goes from position ${position} to ${entry.seq}`);
      position = entry.seq;
    }
    if (page.changesets.length === 0) {
      if (position !== page.head) found.push(`the log ends at ${page.head}, and its last changeset is at ${position}`);
      break;
    }
  }

  const ids = [...state.identities.keys()];
  const step = Math.max(1, Math.floor(ids.length / sample));
  for (let index = 0; index < ids.length; index += step) {
    const id = ids[index]!;
    const view = await client.identity(id);
    if (view.current !== id) found.push(`identity ${id} is listed, and stands for ${view.current}`);
    const named = view.members.map((member) => member.record).sort();
    const pointing = [...state.placed].filter(([, identity]) => identity === id).map(([record]) => record);
    if (named.join() !== pointing.sort().join()) {
      found.push(`identity ${id} names ${named.length} members, and ${pointing.length} records name it`);
    }
  }

  for (const constraint of await client.constraints()) {
    const a = state.placed.get(constraint.a);
    const b = state.placed.get(constraint.b);
    if (constraint.kind === "cannot_link" && a != null && a === b) {
      found.push(`${constraint.a} and ${constraint.b} cannot be linked, and both are in ${a}`);
    }
    if (constraint.kind === "must_link" && a != null && b != null && a !== b) {
      found.push(`${constraint.a} and ${constraint.b} must be linked, and are in ${a} and ${b}`);
    }
  }
  return found;
}
