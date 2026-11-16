// A client for the Spool HTTP API.
//
// The client is thin: each method is one call to the server, and the server
// does the work. It has no dependencies and uses the `fetch` of the runtime
// it runs in.

import type {
  Changeset,
  ChangesetId,
  ChangesetList,
  Committed,
  DraftRecord,
  DraftStatus,
  DraftSummary,
  EntityType,
  ErrorCode,
  HistoryEntry,
  IdentityId,
  IdentityList,
  IdentityView,
  Info,
  IngestRequest,
  Judgement,
  MintedIds,
  ObservationId,
  ObservationList,
  ObservationRecord,
  Rejection,
  ReviewRecord,
  ReviewStatus,
  RunId,
  RunKind,
  RunRecord,
  RunStatus,
  Seq,
  StartRunRequest,
  TypeResolution,
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
}

export interface IdentityFilter {
  entity_type?: EntityType;
  /** The `next` of the page before. */
  after?: IdentityId;
  limit?: number;
}

export interface ObservationFilter {
  entity_type?: EntityType;
  source?: string;
  key?: string;
  /** Only observations that have no identity. */
  unassigned?: boolean;
  /** The `next` of the page before. */
  after?: ObservationId;
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
   * Adds rows as observations. A row that was ingested before, with the same
   * source, key and contents, is not added again.
   *
   * The run that comes back may still be `running` if the server's wait ran
   * out first: pass it to `waitForRun`.
   */
  async ingest(request: IngestRequest): Promise<RunRecord> {
    const { run } = await this.json<{ run: RunRecord }>("POST", "/v1/ingest", { json: request });
    return run;
  }

  /** As `ingest`, with the rows as an Arrow IPC stream. */
  async ingestArrow(options: IngestArrowOptions, rows: ArrowBody): Promise<RunRecord> {
    const { run } = await this.json<{ run: RunRecord }>("POST", "/v1/ingest", {
      query: { ...options },
      arrow: rows,
    });
    return run;
  }

  // Runs

  /**
   * Matches the new observations of an entity type.
   *
   * With no strategy, the server runs the project's rules and reconciles:
   * the run comes back `succeeded`, or `drafted` if `commit` is `false`, or
   * `running` if the server's wait ran out first. `null` means there was
   * nothing new to match.
   *
   * With a strategy of the caller's own, the run comes back `open`: read its
   * candidates, submit judgements, and reconcile it.
   */
  async startRun(request: StartRunRequest): Promise<RunRecord | null> {
    const { run } = await this.json<{ run: RunRecord | null }>("POST", "/v1/runs", { json: request });
    return run;
  }

  async run(run: RunId): Promise<RunRecord> {
    const response = await this.json<{ run: RunRecord }>("GET", `/v1/runs/${run}`);
    return response.run;
  }

  /** Runs, newest first. */
  async runs(filter: RunFilter = {}): Promise<RunRecord[]> {
    const { runs } = await this.json<{ runs: RunRecord[] }>("GET", "/v1/runs", { query: { ...filter } });
    return runs;
  }

  /**
   * Waits until the server has stopped working on a run, and returns it. With
   * a timeout, returns the run as it is when the time is up.
   */
  async waitForRun(run: RunId, options: WaitOptions = {}): Promise<RunRecord> {
    const deadline = options.timeoutMs === undefined ? undefined : Date.now() + options.timeoutMs;
    for (;;) {
      const left = deadline === undefined ? POLL_MS : Math.max(0, deadline - Date.now());
      const { run: record } = await this.json<{ run: RunRecord }>("GET", `/v1/runs/${run}`, {
        query: { wait_ms: Math.min(left, POLL_MS) },
      });
      if (record.status !== "running" || (deadline !== undefined && Date.now() >= deadline)) {
        return record;
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

  /** Gives up an open run. Its observations are still new to the next one. */
  async cancelRun(run: RunId): Promise<RunRecord> {
    const response = await this.json<{ run: RunRecord }>("POST", `/v1/runs/${run}/cancel`);
    return response.run;
  }

  // Drafts

  async drafts(filter: DraftFilter = {}): Promise<DraftSummary[]> {
    const { drafts } = await this.json<{ drafts: DraftSummary[] }>("GET", "/v1/drafts", { query: { ...filter } });
    return drafts;
  }

  /** A draft with its changeset. */
  draft(draft: ChangesetId): Promise<DraftRecord> {
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
   */
  commit(changeset: Changeset): Promise<Committed> {
    return this.json("POST", "/v1/changesets", { json: changeset });
  }

  /** New IDs, for a caller that writes a changeset of its own. */
  mintIds(counts: { identities?: number; changesets?: number }): Promise<MintedIds> {
    return this.json("POST", "/v1/ids", { json: counts });
  }

  // Lookups

  /** A page of active identities. Pass `next` as `after` for the page after. */
  identities(filter: IdentityFilter = {}): Promise<IdentityList> {
    return this.json("GET", "/v1/identities", { query: { ...filter } });
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

  /** A page of observations. Pass `next` as `after` for the page after. */
  observations(filter: ObservationFilter = {}): Promise<ObservationList> {
    return this.json("GET", "/v1/observations", { query: { ...filter } });
  }

  observation(observation: ObservationId): Promise<ObservationRecord> {
    return this.json("GET", `/v1/observations/${observation}`);
  }

  /**
   * The identity of each observation, in the order asked. `null` for an
   * observation that has none or is not known.
   */
  async identitiesOf(observations: ObservationId[]): Promise<Array<IdentityId | null>> {
    const { identities } = await this.json<{ identities: Array<IdentityId | null> }>(
      "POST",
      "/v1/observations/identities",
      { json: { observations } },
    );
    return identities;
  }

  /** What reconciling would not decide. */
  async reviewItems(filter: ReviewFilter = {}): Promise<ReviewRecord[]> {
    const { items } = await this.json<{ items: ReviewRecord[] }>("GET", "/v1/review", { query: { ...filter } });
    return items;
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
