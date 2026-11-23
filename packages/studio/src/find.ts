// Finding a thing from what a person has to hand: an ID out of a log line,
// or the source and key of a row.

import { SpoolError } from "./api.ts";
import type { IdentityView, RecordEntry, SpoolClient } from "./api.ts";

const ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function isId(text: string): boolean {
  return ID.test(text);
}

/**
 * Whether text could name one thing: an ID, or `<source>:<key>`. A key is
 * whatever the source calls its row, so it can hold spaces and colons.
 */
export function isReference(text: string): boolean {
  if (isId(text)) return true;
  const colon = text.indexOf(":");
  return colon >= 1 && colon < text.length - 1 && !/\s/.test(text.slice(0, colon));
}

/** The answer of a call, or `null` if the server has no such thing. */
export async function found<T>(call: Promise<T>): Promise<T | null> {
  try {
    return await call;
  } catch (error) {
    if (error instanceof SpoolError && error.code === "not_found") return null;
    throw error;
  }
}

/**
 * The record a reference names: an ID, or `<source>:<key>`, which names
 * the version of the row that nothing has replaced. `null` if there is none.
 */
export async function findRecord(client: SpoolClient, reference: string): Promise<RecordEntry | null> {
  const text = reference.trim();
  if (isId(text)) return found(client.record(text));
  const colon = text.indexOf(":");
  if (colon < 1) return null;
  const source = text.slice(0, colon);
  const key = text.slice(colon + 1);
  // Versions are listed by ID, in pages, and the latest can be on any of
  // them: it is the one no other replaced.
  let last: RecordEntry | null = null;
  let after: string | undefined;
  for (;;) {
    const page = await client.records({ source, key, after });
    for (const version of page.records) {
      if (!version.superseded_by) return version;
      last = version;
    }
    if (!page.next) return last;
    after = page.next;
  }
}

/**
 * The identity a reference names, as it stands now: an identity ID, or an
 * record that belongs to it, by ID or as `<source>:<key>`. An identity
 * that was merged away names the one that stands for it. `null` if there is
 * none, or the record has no identity.
 */
export async function findIdentity(client: SpoolClient, reference: string): Promise<IdentityView | null> {
  const text = reference.trim();
  if (text === "") return null;
  let view = isId(text) ? await found(client.identity(text)) : null;
  if (view === null) {
    const record = await findRecord(client, text);
    if (!record?.identity) return null;
    view = await client.identity(record.identity);
  }
  return view.current === view.id ? view : client.identity(view.current);
}

/** A thing that has a screen of its own: which kind of thing, and its ID. */
export interface Found {
  kind: "record" | "identity" | "document" | "draft";
  id: string;
}

/**
 * What a reference names that has a screen, or `null` if nothing has that
 * name. An ID is looked for among the records, the identities, the
 * documents and the drafts.
 */
export async function findScreen(client: SpoolClient, reference: string): Promise<Found | null> {
  const text = reference.trim();
  if (text === "") return null;
  if (isId(text)) {
    const [record, identity, document, draft] = await Promise.all([
      found(client.record(text)),
      found(client.identity(text)),
      found(client.document(text)),
      found(client.draft(text)),
    ]);
    if (record) return { kind: "record", id: text };
    if (identity) return { kind: "identity", id: text };
    if (document) return { kind: "document", id: text };
    if (draft) return { kind: "draft", id: text };
    return null;
  }
  const record = await findRecord(client, text);
  if (record) return { kind: "record", id: record.id };
  const colon = text.indexOf(":");
  if (colon < 1) return null;
  const { documents } = await client.documents({
    source: text.slice(0, colon),
    key: text.slice(colon + 1),
    latest: true,
    limit: 1,
  });
  return documents[0] ? { kind: "document", id: documents[0].id } : null;
}
