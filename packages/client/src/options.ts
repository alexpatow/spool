// What the client's methods take besides the server's own request types:
// filters of the lists, the orders they are sorted in, and options.

import type {
  DocumentId,
  DraftStatus,
  EntityType,
  IdentityId,
  RecordId,
  ReviewStatus,
  RunKind,
  RunStatus,
} from "./types.ts";

/** Arrow IPC stream bytes, however the caller holds them. */
export type ArrowBody = Uint8Array | ArrayBuffer | Blob;

export interface LoadArrowOptions {
  source: string;
  entity_type: string;
  /** As `replace` of a `LoadRequest`. */
  replace?: boolean;
  wait_ms?: number;
}

/**
 * Which way a list runs. A list given `order` without `sort` turns its own
 * order around.
 */
export type SortOrder = "asc" | "desc";

/**
 * What a list read in pages, or cut at `limit`, is sorted by. The whole of
 * it is sorted, and a page continues after its `after` in that order.
 */
export interface Sorted<By extends string> {
  sort?: By;
  order?: SortOrder;
}

export type RunSort = "run" | "kind" | "status" | "details" | "started_by" | "started";
export type DocumentSort = "id" | "key" | "source" | "length" | "loaded";
export type RecordSort = "id" | "source" | "key" | "label" | "entity_type" | "identity";
export type IdentitySort = "id" | "label" | "entity_type" | "records" | "sources" | "created";
export type LogSort = "seq" | "actor" | "operations" | "run" | "reason";

export interface RunFilter extends Sorted<RunSort> {
  kind?: RunKind;
  status?: RunStatus;
  entity_type?: EntityType;
  /** At most this many, newest first or the first in the order asked for. */
  limit?: number;
}

export interface DraftFilter {
  status?: DraftStatus;
  entity_type?: EntityType;
}

export interface ReviewFilter {
  status?: ReviewStatus;
  entity_type?: EntityType;
  /** Only items that name this record. */
  record?: RecordId;
}

export interface ConstraintFilter {
  /** Only constraints that name this record. */
  record?: RecordId;
  /** Only constraints that name a member of this identity. */
  identity?: IdentityId;
  /** Also constraints that were removed. Default: only those in force. */
  removed?: boolean;
}

export interface IdentityFilter extends Sorted<IdentitySort> {
  entity_type?: EntityType;
  /**
   * Only identities with a member that holds every word of this text: in a
   * value of its payload, in the name of its source or in its key, whatever
   * the case and the accents of the letters.
   */
  q?: string;
  /**
   * Only these identities, which is how a caller that holds IDs reads what
   * they are called. One that was merged away is left out. At most 1000.
   */
  ids?: IdentityId[];
  /** The `next` of the page before. */
  after?: IdentityId;
  limit?: number;
}

export interface DocumentFilter extends Sorted<DocumentSort> {
  source?: string;
  key?: string;
  /** Only the latest version of each document. Default: every version. */
  latest?: boolean;
  /** The `next` of the page before. */
  after?: DocumentId;
  limit?: number;
}

export interface RecordFilter extends Sorted<RecordSort> {
  entity_type?: EntityType;
  source?: string;
  key?: string;
  /** Only the mentions an extractor found in this version of a document. */
  document?: DocumentId;
  /** Only the members this identity has now. */
  identity?: IdentityId;
  /**
   * Only records that hold every word of this text: in a value of the
   * payload, in the name of the source or in the key, whatever the case and
   * the accents of the letters.
   */
  q?: string;
  /** Only records that have no identity. */
  unassigned?: boolean;
  /** Only the latest version of each row or mention. Default: every version. */
  latest?: boolean;
  /** The `next` of the page before. */
  after?: RecordId;
  limit?: number;
}

export interface WaitOptions {
  /** How long to wait for the run to end, in milliseconds. Default: no limit. */
  timeoutMs?: number;
}
