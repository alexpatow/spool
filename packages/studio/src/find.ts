// Finding a thing from what a person has to hand: an ID out of a log line,
// or the source and key of a row.

import { SpoolError } from "./api.ts";
import type { ObservationRecord, SpoolClient } from "./api.ts";
import { href } from "./router.ts";

const ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isId(text: string): boolean {
  return ID.test(text);
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
 * The observation a reference names: an ID, or `<source>:<key>`, which names
 * the version of the row that nothing has replaced. `null` if there is none.
 */
export async function findObservation(client: SpoolClient, reference: string): Promise<ObservationRecord | null> {
  const text = reference.trim();
  if (isId(text)) return found(client.observation(text));
  const colon = text.indexOf(":");
  if (colon < 1) return null;
  const source = text.slice(0, colon);
  const key = text.slice(colon + 1);
  // Versions are listed by ID, in pages, and the latest can be on any of
  // them: it is the one no other replaced.
  let last: ObservationRecord | null = null;
  let after: string | undefined;
  for (;;) {
    const page = await client.observations({ source, key, after });
    for (const version of page.observations) {
      if (!version.superseded_by) return version;
      last = version;
    }
    if (!page.next) return last;
    after = page.next;
  }
}

/**
 * The screen that shows what a reference names, or `null` if nothing has
 * that name. An ID is looked for among the observations, the identities,
 * the documents and the drafts.
 */
export async function findScreen(client: SpoolClient, reference: string): Promise<string | null> {
  const text = reference.trim();
  if (text === "") return null;
  if (isId(text)) {
    const [observation, identity, document, draft] = await Promise.all([
      found(client.observation(text)),
      found(client.identity(text)),
      found(client.document(text)),
      found(client.draft(text)),
    ]);
    if (observation) return href(["observations", text]);
    if (identity) return href(["identities", text]);
    if (document) return href(["documents", text]);
    if (draft) return href(["drafts", text]);
    return null;
  }
  const observation = await findObservation(client, text);
  if (observation) return href(["observations", observation.id]);
  const colon = text.indexOf(":");
  if (colon < 1) return null;
  const { documents } = await client.documents({
    source: text.slice(0, colon),
    key: text.slice(colon + 1),
    latest: true,
    limit: 1,
  });
  return documents[0] ? href(["documents", documents[0].id]) : null;
}
