// A client for the Spool HTTP API.
//
// The client is thin: each method is one call to the server, and the server
// does the work. It has no dependencies and uses the `fetch` of the runtime
// it runs in.

import type {
  Authored,
  ChangesetId,
  ChangesetList,
  CommitRequest,
  Committed,
  ConstraintEntry,
  DismissRequest,
  DocumentId,
  DocumentList,
  DocumentEntry,
  DraftEntry,
  DraftStatus,
  DraftSummary,
  EntityType,
  ErrorCode,
  Explanation,
  ExtractRequest,
  HistoryEntry,
  IdentityId,
  IdentityList,
  IdentityView,
  Info,
  IngestRequest,
  Judgement,
  MintedIds,
  NewDocument,
  RecordId,
  RecordList,
  RecordEntry,
  Rejection,
  ReviewCase,
  ReviewEntry,
  ReviewStatus,
  RunId,
  RunKind,
  RunEntry,
  RunStatus,
  Seq,
  SplitReport,
  SplitRequest,
  StartRunRequest,
  TypeResolution,
  UndoRequest,
} from "./types.ts";

export type * from "./types.ts";

/** The media type of an Arrow IPC stream. */
export const ARROW_STREAM = "application/vnd.apache.arrow.stream";

/** The part of `fetch` the client uses. */
export type Fetch = (input: URL, init: RequestInit) => Promise<Response>;

export interface SpoolClientOptions {
  /** Where the server is, for example `http://localhost:7765`. */
  url: string;
  /** The server's bearer token. */
  token: string;
  /** The `fetch` to call with. Defaults to the runtime's. */
  fetch?: Fetch;
}

/** A call the server answered with an error. */
export class SpoolError extends Error {
  /** The HTTP status. */
  readonly status: number;
  readonly code: ErrorCode;
  /** With `rejected`: the operations of the changeset that broke a rule. */
  readonly rejections: Rejection[];

  constructor(status: number, code: ErrorCode, message: string, rejections: Rejection[] = []) {
    super(message);
    this.name = "SpoolError";
    this.status = status;
    this.code = code;
    this.rejections = rejections;
  }
}

/** Arrow IPC stream bytes, however the caller holds them. */
export type ArrowBody = Uint8Array | ArrayBuffer | Blob;

export interface IngestArrowOptions {
  source: string;
  entity_type: string;
  /** The column that holds each row's key within the source. */
  key?: string;
  /** A column of JSON text that says where each row was found. */
  evidence?: string;
  wait_ms?: number;
}

export interface RunFilter {
  kind?: RunKind;
  status?: RunStatus;
  entity_type?: EntityType;
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

export interface IdentityFilter {
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

export interface DocumentFilter {
  source?: string;
  key?: string;
  /** Only the latest version of each document. Default: every version. */
  latest?: boolean;
  /** The `next` of the page before. */
  after?: DocumentId;
  limit?: number;
}

export interface RecordFilter {
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
  /** The `next` of the page before. */
  after?: RecordId;
  limit?: number;
}

export interface WaitOptions {
  /** How long to wait for the run to end, in milliseconds. Default: no limit. */
  timeoutMs?: number;
}

type Query = Record<string, string | number | boolean | undefined | null>;

interface Call {
  query?: Query;
  /** A JSON body. */
  json?: unknown;
  /** An Arrow IPC stream body. */
  arrow?: ArrowBody;
  accept?: string;
}

// How long one read of a run waits on the server before the client asks again.
const POLL_MS = 20_000;

export class SpoolClient {
  private readonly url: string;
  private readonly token: string;
  private readonly fetch: Fetch;

  constructor(options: SpoolClientOptions) {
    this.url = options.url.replace(/\/+$/, "");
    this.token = options.token;
    this.fetch = options.fetch ?? ((input, init) => fetch(input, init));
  }

  // The store

  /** What the store holds, and the versions the server speaks. */
  info(): Promise<Info> {
    return this.json("GET", "/v1/info");
  }

  /** Whether the server is up. Needs no token. */
  async healthy(): Promise<boolean> {
    try {
      const response = await this.fetch(new URL(`${this.url}/healthz`), { method: "GET" });
      return response.ok;
    } catch {
      return false;
    }
  }

  // Ingest

  /**
   * Adds rows as records. A row that was ingested before, with the same
   * source, key and contents, is not added again.
   *
   * The run that comes back may still be `running` if the server's wait ran
   * out first: pass it to `waitForRun`.
   */
  async ingest(request: IngestRequest): Promise<RunEntry> {
    const { run } = await this.json<{ run: RunEntry }>("POST", "/v1/ingest", { json: request });
    return run;
  }

  /** As `ingest`, with the rows as an Arrow IPC stream. */
  async ingestArrow(options: IngestArrowOptions, rows: ArrowBody): Promise<RunEntry> {
    const { run } = await this.json<{ run: RunEntry }>("POST", "/v1/ingest", {
      query: { ...options },
      arrow: rows,
    });
    return run;
  }

  // Documents and extraction

  /**
   * Stores documents from one source, for an extractor to read. A document
   * that was stored before, with the same source, key and text, is not
   * stored again, and a changed text under the same key is the document's
   * next version. The run that comes back is an ingest run.
   */
  async addDocuments(source: string, documents: NewDocument[], options: { wait_ms?: number } = {}): Promise<RunEntry> {
    const { run } = await this.json<{ run: RunEntry }>("POST", "/v1/documents", {
      json: { source, documents, ...options },
    });
    return run;
  }

  /** A page of documents, without their text. Pass `next` as `after` for the page after. */
  documents(filter: DocumentFilter = {}): Promise<DocumentList> {
    return this.json("GET", "/v1/documents", { query: { ...filter } });
  }

  /** One version of a document, with its text and each time an extractor read it. */
  document(document: DocumentId): Promise<DocumentEntry> {
    return this.json("GET", `/v1/documents/${document}`);
  }

  /**
   * Runs one of the project's extractors, which `info` lists, over the
   * latest version of every document it has not read. What it finds is
   * stored as records, which `startRun` then matches like any other;
   * `extraction` on the run that comes back counts them.
   *
   * The run may still be `running` if the server's wait ran out first: pass
   * it to `waitForRun`. If the extractor fails, the call throws a
   * `SpoolError` with the code `extractor_failed`, and nothing is stored.
   */
  async extract(extractor: string, options: Omit<ExtractRequest, "extractor"> = {}): Promise<RunEntry> {
    const { run } = await this.json<{ run: RunEntry }>("POST", "/v1/extractions", {
      json: { extractor, ...options },
    });
    return run;
  }

  // Runs

  /**
   * Matches the new records of an entity type.
   *
   * With no strategy, the server runs the project's rules and reconciles:
   * the run comes back `succeeded`, or `drafted` if `commit` is `false`, or
   * `running` if the server's wait ran out first. `null` means there was
   * nothing new to match. Naming one of the project's user-code strategies,
   * which `info` lists for each entity type, runs that instead; if it fails,
   * the call throws a `SpoolError` with the code `strategy_failed`.
   *
   * With `rematch`, every record is matched again. Identities are left
   * as they are, and `resolution.suggestions` lists the merges and splits
   * the strategy would make of them.
   *
   * With any other strategy name, the run comes back `open`: read its
   * candidates, submit judgements, and reconcile it. The server runs no
   * code a caller sends.
   */
  async startRun(request: StartRunRequest): Promise<RunEntry | null> {
    const { run } = await this.json<{ run: RunEntry | null }>("POST", "/v1/runs", { json: request });
    return run;
  }

  async run(run: RunId): Promise<RunEntry> {
    const response = await this.json<{ run: RunEntry }>("GET", `/v1/runs/${run}`);
    return response.run;
  }

  /** Runs, newest first. */
  async runs(filter: RunFilter = {}): Promise<RunEntry[]> {
    const { runs } = await this.json<{ runs: RunEntry[] }>("GET", "/v1/runs", { query: { ...filter } });
    return runs;
  }

  /**
   * Waits until the server has stopped working on a run, and returns it. With
   * a timeout, returns the run as it is when the time is up.
   */
  async waitForRun(run: RunId, options: WaitOptions = {}): Promise<RunEntry> {
    const deadline = options.timeoutMs === undefined ? undefined : Date.now() + options.timeoutMs;
    for (;;) {
      const left = deadline === undefined ? POLL_MS : Math.max(0, deadline - Date.now());
      const { run: entry } = await this.json<{ run: RunEntry }>("GET", `/v1/runs/${run}`, {
        query: { wait_ms: Math.min(left, POLL_MS) },
      });
      if (entry.status !== "running" || (deadline !== undefined && Date.now() >= deadline)) {
        return entry;
      }
    }
  }

  /**
   * The candidate pairs of an open run. Each has `left_id`, `right_id`, and
   * every feature column prefixed with `left_` and with `right_`.
   */
  async candidates(run: RunId): Promise<Array<Record<string, unknown>>> {
    const { candidates } = await this.json<{ candidates: Array<Record<string, unknown>> }>(
      "GET",
      `/v1/runs/${run}/candidates`,
    );
    return candidates;
  }

  /** As `candidates`, as the bytes of an Arrow IPC stream. */
  async candidatesArrow(run: RunId): Promise<Uint8Array> {
    const response = await this.send("GET", `/v1/runs/${run}/candidates`, { accept: ARROW_STREAM });
    return new Uint8Array(await response.arrayBuffer());
  }

  /** Stores judgements on an open run, and returns how many were stored. */
  async submitJudgements(run: RunId, judgements: Judgement[]): Promise<number> {
    const { stored } = await this.json<{ stored: number }>("POST", `/v1/runs/${run}/judgements`, {
      json: { judgements },
    });
    return stored;
  }

  /**
   * As `submitJudgements`, with the judgements as an Arrow IPC stream with
   * the columns `left_id`, `right_id`, `verdict`, and optionally `score` and
   * `detail`.
   */
  async submitJudgementsArrow(run: RunId, judgements: ArrowBody): Promise<number> {
    const { stored } = await this.json<{ stored: number }>("POST", `/v1/runs/${run}/judgements`, {
      arrow: judgements,
    });
    return stored;
  }

  /**
   * Turns a run's judgements into a changeset, and commits it. With
   * `commit: false` the changeset is stored as a draft for review instead,
   * and `draft` in the result names it.
   */
  reconcile(run: RunId, options: { commit?: boolean } = {}): Promise<TypeResolution> {
    return this.json("POST", `/v1/runs/${run}/reconcile`, { json: options });
  }

  /** Gives up an open run. Its records are still new to the next one. */
  async cancelRun(run: RunId): Promise<RunEntry> {
    const response = await this.json<{ run: RunEntry }>("POST", `/v1/runs/${run}/cancel`);
    return response.run;
  }

  // Drafts

  async drafts(filter: DraftFilter = {}): Promise<DraftSummary[]> {
    const { drafts } = await this.json<{ drafts: DraftSummary[] }>("GET", "/v1/drafts", { query: { ...filter } });
    return drafts;
  }

  /** A draft with its changeset. */
  draft(draft: ChangesetId): Promise<DraftEntry> {
    return this.json("GET", `/v1/drafts/${draft}`);
  }

  /**
   * Commits a draft. If state has moved under it, this throws a `SpoolError`
   * with the code `rejected` whose `rejections` name the operations that no
   * longer hold, and the draft stays open.
   */
  approveDraft(draft: ChangesetId): Promise<Committed> {
    return this.json("POST", `/v1/drafts/${draft}/approve`);
  }

  /** Closes a draft without committing it. */
  rejectDraft(draft: ChangesetId, reason?: string): Promise<DraftSummary> {
    return this.json("POST", `/v1/drafts/${draft}/reject`, { json: reason === undefined ? {} : { reason } });
  }

  // Changesets

  /** A page of the log, oldest first, and the position of its end. */
  changesets(options: { after?: Seq; limit?: number } = {}): Promise<ChangesetList> {
    return this.json("GET", "/v1/changesets", { query: { ...options } });
  }

  /**
   * Validates a changeset against current state and commits it. A changeset
   * that breaks a rule throws a `SpoolError` with the code `rejected`.
   *
   * Merging two identities, moving a record and recording or removing
   * a constraint are each a changeset of one operation. Without an `id` the
   * server mints one, and without a `base` the changeset is taken to have
   * been written against the head of the log.
   */
  commit(changeset: CommitRequest): Promise<Committed> {
    return this.json("POST", "/v1/changesets", { json: changeset });
  }

  /**
   * Takes back the changeset at a log position with a new changeset, which
   * is validated like any other: it is rejected if state has moved on in a
   * way the undo would trample. With `commit: false` the undo is returned
   * and not made.
   */
  undo(seq: Seq, request: UndoRequest): Promise<Authored> {
    return this.json("POST", `/v1/changesets/${seq}/undo`, { json: request });
  }

  /**
   * Divides an identity in two. The larger side keeps the ID, and unless
   * `keep_apart` is `false` a `cannot_link` constraint is recorded between
   * the sides.
   */
  split(identity: IdentityId, request: SplitRequest): Promise<SplitReport> {
    return this.json("POST", `/v1/identities/${identity}/split`, { json: request });
  }

  /** The constraints in force: pairs of records that must or must not share an identity. */
  async constraints(filter: ConstraintFilter = {}): Promise<ConstraintEntry[]> {
    const { constraints } = await this.json<{ constraints: ConstraintEntry[] }>("GET", "/v1/constraints", {
      query: { ...filter },
    });
    return constraints;
  }

  /** New IDs, for a caller that writes a changeset of its own. */
  mintIds(counts: { identities?: number; changesets?: number }): Promise<MintedIds> {
    return this.json("POST", "/v1/ids", { json: counts });
  }

  // Lookups

  /** A page of active identities. Pass `next` as `after` for the page after. */
  identities(filter: IdentityFilter = {}): Promise<IdentityList> {
    return this.json("GET", "/v1/identities", { query: { ...filter, ids: filter.ids?.join(",") } });
  }

  /** An identity and its members. */
  identity(identity: IdentityId): Promise<IdentityView> {
    return this.json("GET", `/v1/identities/${identity}`);
  }

  /** The changesets that touched an identity, oldest first. */
  async identityHistory(identity: IdentityId): Promise<HistoryEntry[]> {
    const { history } = await this.json<{ history: HistoryEntry[] }>("GET", `/v1/identities/${identity}/history`);
    return history;
  }

  /** A page of records. Pass `next` as `after` for the page after. */
  records(filter: RecordFilter = {}): Promise<RecordList> {
    return this.json("GET", "/v1/records", { query: { ...filter } });
  }

  record(record: RecordId): Promise<RecordEntry> {
    return this.json("GET", `/v1/records/${record}`);
  }

  /**
   * The identity of each record, in the order asked. `null` for an
   * record that has none or is not known.
   */
  async identitiesOf(records: RecordId[]): Promise<Array<IdentityId | null>> {
    const { identities } = await this.json<{ identities: Array<IdentityId | null> }>(
      "POST",
      "/v1/records/identities",
      { json: { records } },
    );
    return identities;
  }

  /**
   * Why two records share an identity, or why they do not: where each
   * is and what placed it there, what each run that compared them concluded,
   * the constraints and review items that name them, and what the project's
   * rules say about the pair as it is now. `summary` says it in sentences.
   */
  explain(a: RecordId, b: RecordId): Promise<Explanation> {
    return this.json("GET", "/v1/explain", { query: { a, b } });
  }

  /** What reconciling would not decide. */
  async reviewItems(filter: ReviewFilter = {}): Promise<ReviewEntry[]> {
    const { items } = await this.json<{ items: ReviewEntry[] }>("GET", "/v1/review", { query: { ...filter } });
    return items;
  }

  reviewItem(item: number): Promise<ReviewEntry> {
    return this.json("GET", `/v1/review/${item}`);
  }

  /**
   * A review item with what a reviewer reads to decide it: the records
   * it holds back, the identities they matched with their members, and what
   * was judged between them.
   */
  reviewCase(item: number): Promise<ReviewCase> {
    return this.json("GET", `/v1/review/${item}/case`);
  }

  /**
   * Closes a review item without giving its records an identity. Later
   * runs leave them alone.
   */
  dismissReview(item: number, request: DismissRequest): Promise<ReviewEntry> {
    return this.json("POST", `/v1/review/${item}/dismiss`, { json: request });
  }

  // The wire

  private async json<T>(method: string, path: string, call: Call = {}): Promise<T> {
    const response = await this.send(method, path, { accept: "application/json", ...call });
    return (await response.json()) as T;
  }

  private async send(method: string, path: string, call: Call): Promise<Response> {
    const url = new URL(this.url + path);
    for (const [name, value] of Object.entries(call.query ?? {})) {
      if (value !== undefined && value !== null) url.searchParams.set(name, String(value));
    }
    const headers: Record<string, string> = { Authorization: `Bearer ${this.token}` };
    if (call.accept !== undefined) headers.Accept = call.accept;
    let body: BodyInit | undefined;
    if (call.arrow !== undefined) {
      headers["Content-Type"] = ARROW_STREAM;
      body = call.arrow as BodyInit;
    } else if (call.json !== undefined) {
      headers["Content-Type"] = "application/json";
      body = JSON.stringify(call.json);
    }

    const response = await this.fetch(url, { method, headers, body });
    if (response.ok) return response;
    throw await failure(response);
  }
}

async function failure(response: Response): Promise<SpoolError> {
  const text = await response.text();
  try {
    const { error } = JSON.parse(text) as {
      error: { code: ErrorCode; message: string; rejections?: Rejection[] | null };
    };
    return new SpoolError(response.status, error.code, error.message, error.rejections ?? []);
  } catch {
    // Not the server's own error: something between here and it answered.
    const code: ErrorCode = response.status === 401 ? "unauthorized" : response.status === 404 ? "not_found" : "internal";
    return new SpoolError(response.status, code, text.trim() || `the server answered ${response.status}`);
  }
}
