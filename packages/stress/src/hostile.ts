// What a server makes of requests nobody should send.
//
//   bun src/hostile.ts
//
// Each case is one request and the status the server has to answer it with.
// Whatever is sent, the server may not fail on its own account (a status of
// 500 or above), every refusal has to be the documented error body, and
// afterwards the server still has to work and its store has to verify.

import { appendFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { scratch, serve, TOKEN, verify } from "./server.ts";
import { copyCompanies } from "./write.ts";

interface Case {
  name: string;
  /**
   * The status the server answers with. `ANY`: the case is here to show the
   * server survives it, and what it should answer is not settled.
   */
  status: number;
  method?: string;
  path: string;
  /** A body sent as JSON. */
  body?: unknown;
  /** A body sent as it is. */
  raw?: string | Uint8Array<ArrayBuffer>;
  headers?: Record<string, string>;
  /** The bearer token. Null: none is sent. Default: the server's. */
  token?: string | null;
}

const ANY = 0;

export interface HostileResult {
  cases: number;
  /** Everything that was wrong. Empty when nothing was. */
  faults: string[];
}

function all(): Case[] {
  const cases: Case[] = [];
  const ingest = (name: string, status: number, body: unknown) =>
    cases.push({ name: `ingest: ${name}`, status, method: "POST", path: "/v1/ingest", body });
  const ok = { source: "crm", entity_type: "company" };
  const sql = `x'; DROP TABLE spool.record; --`;
  const long = "a".repeat(1_000_000);

  ingest("ok row", 200, { ...ok, rows: [{ id: "1", name: "Acme Ltd", country: "GB" }] });
  ingest("no rows", 200, { ...ok, rows: [] });
  ingest("rows not objects", 400, { ...ok, rows: [1, "a", null] });
  ingest("row is array", 400, { ...ok, rows: [[1, 2]] });
  ingest("nested object value", 400, { ...ok, rows: [{ id: "2", name: { a: 1 } }] });
  ingest("array value", 400, { ...ok, rows: [{ id: "3", name: ["a", "b"] }] });
  ingest("null value", 200, { ...ok, rows: [{ id: "4", name: null }] });
  ingest("number and bool values", 200, { ...ok, rows: [{ id: "5", name: 12, country: true, city: 1.5e300 }] });
  ingest("key missing in a row", 200, { ...ok, rows: [{ name: "No Key" }] });
  ingest("key null", 200, { ...ok, rows: [{ id: null, name: "Null Key" }] });
  ingest("key empty", 200, { ...ok, rows: [{ id: "", name: "Empty Key" }] });
  ingest("key number", 200, { ...ok, rows: [{ id: 7, name: "Number Key" }] });
  ingest("key number vs text 7", 200, { ...ok, rows: [{ id: "7", name: "Number Key" }] });
  ingest("key bool", 200, { ...ok, rows: [{ id: true, name: "Bool Key" }] });
  ingest("key float", 200, {
    ...ok,
    rows: [
      { id: 1.0, name: "Float Key" },
      { id: 1, name: "Int Key" },
    ],
  });
  ingest("duplicate keys in batch same", 200, {
    ...ok,
    rows: [
      { id: "d", name: "Dup" },
      { id: "d", name: "Dup" },
    ],
  });
  ingest("duplicate keys in batch differ", 200, {
    ...ok,
    rows: [
      { id: "e", name: "Dup One" },
      { id: "e", name: "Dup Two" },
    ],
  });
  ingest("key column named in the call", 400, { ...ok, key: "id", rows: [{ id: "k2" }] });
  ingest("key 1MB", 200, { ...ok, rows: [{ id: long, name: "Long Key" }] });
  ingest("value 1MB", 200, { ...ok, rows: [{ id: "long", name: long }] });
  ingest("key with NUL", 200, { ...ok, rows: [{ id: "a\u0000b", name: "Nul Key" }] });
  ingest("value with NUL", 200, { ...ok, rows: [{ id: "nul", name: "Nul\u0000Value" }] });
  ingest("key sql", 200, { ...ok, rows: [{ id: sql, name: sql }] });
  ingest("key unicode", 200, { ...ok, rows: [{ id: "ключ-🚀-鍵", name: "Société 🚀 GmbH" }] });
  // A column the source's file does not list is refused, whatever its name.
  ingest("column not declared", 400, { ...ok, rows: [{ id: "c0", colour: "x" }] });
  ingest("column name empty", 400, { ...ok, rows: [{ id: "c1", "": "x" }] });
  ingest("column name sql", 400, { ...ok, rows: [{ id: "c2", [sql]: "x" }] });
  ingest("column name with quote and dot", 400, { ...ok, rows: [{ id: "c3", 'a"b.c$[0]': "x" }] });
  ingest("column name NUL", 400, { ...ok, rows: [{ id: "c4", "a\u0000b": "x" }] });
  ingest("2000 columns", 400, {
    ...ok,
    rows: [Object.fromEntries([["id", "wide"], ...Array.from({ length: 2000 }, (_, i) => [`col${i}`, "v"])])],
  });
  ingest("rows with different columns", 200, {
    ...ok,
    rows: [{ id: "m1", name: "A" }, { id: "m2", website: "b.example" }, { id: "m3" }],
  });
  ingest("rows with one column of mixed types", 400, {
    ...ok,
    rows: [
      { id: "t1", name: "A" },
      { id: "t2", name: 5 },
      { id: "t3", name: true },
    ],
  });
  ingest("source empty", 400, { ...ok, source: "", rows: [{ id: "s1" }] });
  // A source the project does not declare is refused, whatever its name.
  ingest("source not declared", 400, { ...ok, source: "erp", rows: [{ id: "s0", name: "Erp" }] });
  ingest("source sql", 400, { ...ok, source: sql, rows: [{ id: "s2", name: "Sql Source" }] });
  ingest("source with colon", 400, { ...ok, source: "a:b", rows: [{ id: "s3:x", name: "Colon" }] });
  ingest("source with a path", 400, { ...ok, source: "../crm", rows: [{ id: "s3" }] });
  ingest("source 100k long", 400, { ...ok, source: "s".repeat(100_000), rows: [{ id: "s4" }] });
  ingest("source NUL", 400, { ...ok, source: "a\u0000b", rows: [{ id: "s5" }] });
  ingest("source number", 400, { ...ok, source: 5, rows: [{ id: "s6" }] });
  ingest("entity type unknown", 400, { ...ok, entity_type: "person", rows: [{ id: "u1" }] });
  ingest("entity type sql", 400, { ...ok, entity_type: sql, rows: [{ id: "u2" }] });
  ingest("entity type upper", 400, { ...ok, entity_type: "Company", rows: [{ id: "u3" }] });
  ingest("unknown field", 400, { ...ok, rows: [{ id: "f1" }], extra: 1 });
  // `scans` is a source whose file names `ev` as its evidence column.
  const scans = { source: "scans", entity_type: "company" };
  ingest("evidence not json", 400, { ...scans, rows: [{ id: "ev1", ev: "not json" }] });
  ingest("evidence json", 200, { ...scans, rows: [{ id: "ev2", ev: '{"page": 3}' }] });
  ingest("evidence without a key", 400, { ...scans, rows: [{ ev: '{"page": 3}' }] });
  ingest("evidence column named in the call", 400, { ...ok, evidence: "ev", rows: [{ id: "ev3" }] });
  ingest("wait_ms negative", 400, { ...ok, wait_ms: -1, rows: [{ id: "w1" }] });
  ingest("wait_ms huge", 400, { ...ok, wait_ms: 1e30, rows: [{ id: "w2" }] });
  ingest("wait_ms u64 max", 400, { ...ok, wait_ms: 18446744073709551615, rows: [{ id: "w3" }] });
  cases.push({
    name: "ingest: not json",
    status: 400,
    method: "POST",
    path: "/v1/ingest",
    raw: "{",
    headers: { "content-type": "application/json" },
  });
  cases.push({
    name: "ingest: empty body",
    status: 400,
    method: "POST",
    path: "/v1/ingest",
    raw: "",
    headers: { "content-type": "application/json" },
  });
  cases.push({
    name: "ingest: json top-level array",
    status: 400,
    method: "POST",
    path: "/v1/ingest",
    raw: "[]",
    headers: { "content-type": "application/json" },
  });
  cases.push({
    name: "ingest: invalid utf-8",
    status: 400,
    method: "POST",
    path: "/v1/ingest",
    raw: new Uint8Array([0x7b, 0x22, 0xff, 0xfe, 0x22, 0x7d]),
    headers: { "content-type": "application/json" },
  });
  cases.push({
    name: "ingest: lone surrogate",
    status: 400,
    method: "POST",
    path: "/v1/ingest",
    raw: '{"source":"crm","entity_type":"company","rows":[{"id":"\\ud800"}]}',
    headers: { "content-type": "application/json" },
  });
  cases.push({
    name: "ingest: deep nesting",
    status: 400,
    method: "POST",
    path: "/v1/ingest",
    raw: '{"source":"crm","entity_type":"company","rows":[{"id":' + "[".repeat(100_000) + "]".repeat(100_000) + "}]}",
    headers: { "content-type": "application/json" },
  });
  cases.push({
    name: "ingest: duplicate json keys",
    status: 200,
    method: "POST",
    path: "/v1/ingest",
    raw: '{"source":"crm","entity_type":"company","rows":[{"id":"dk","name":"A","name":"B"}]}',
    headers: { "content-type": "application/json" },
  });
  cases.push({
    name: "ingest: big integer",
    status: 200,
    method: "POST",
    path: "/v1/ingest",
    raw: '{"source":"crm","entity_type":"company","rows":[{"id":"big","name":123456789012345678901234567890123456789012345678901234567890}]}',
    headers: { "content-type": "application/json" },
  });
  cases.push({
    name: "ingest: arrow garbage",
    status: 400,
    method: "POST",
    path: "/v1/ingest?source=crm&entity_type=company",
    raw: "not arrow",
    headers: { "content-type": "application/vnd.apache.arrow.stream" },
  });
  cases.push({
    name: "ingest: arrow empty",
    status: 400,
    method: "POST",
    path: "/v1/ingest?source=crm&entity_type=company",
    raw: "",
    headers: { "content-type": "application/vnd.apache.arrow.stream" },
  });
  cases.push({
    name: "ingest: no content type",
    status: 200,
    method: "POST",
    path: "/v1/ingest",
    raw: JSON.stringify({ ...ok, rows: [{ id: "nct" }] }),
  });
  cases.push({
    name: "ingest: text/plain",
    status: 200,
    method: "POST",
    path: "/v1/ingest",
    raw: JSON.stringify({ ...ok, rows: [{ id: "tp" }] }),
    headers: { "content-type": "text/plain" },
  });

  // Reads with odd addresses and parameters.
  const reads: Array<[status: number, paths: string[]]> = [
    [
      200,
      [
        "/v1/records?source=crm&key=%00",
        `/v1/records?source=${encodeURIComponent(sql)}`,
        "/v1/records?limit=0",
        "/v1/changesets?limit=0",
        "/v1/documents?source=%00",
        "/v1/../healthz",
      ],
    ],
    [
      404,
      [
        "/v1/identities/00000000-0000-0000-0000-000000000000",
        "/v1/changesets/0",
        "/v1/runs/0",
        "/v1/review/0",
        "/v1//info",
        "/v1/info/",
        "/V1/info",
        "/v1/nope",
        "/..%2f..%2fetc%2fpasswd",
        "/studio.js/../../etc/passwd",
        "/%2e%2e/%2e%2e/etc/passwd",
      ],
    ],
    [
      400,
      [
        "/v1/identities/not-a-uuid",
        `/v1/identities/${encodeURIComponent(sql)}`,
        "/v1/records/not-a-uuid",
        "/v1/records/%00",
        "/v1/records?limit=-1",
        "/v1/records?limit=1e99",
        "/v1/records?limit=99999999999999999999",
        "/v1/records?after=garbage",
        "/v1/records?nope=1",
        "/v1/records?source=crm&source=registry",
        "/v1/identities?entity_type=Company",
        `/v1/identities?entity_type=${encodeURIComponent(sql)}`,
        "/v1/identities?after=zzz",
        "/v1/changesets?after=-1",
        "/v1/changesets?after=18446744073709551616",
        "/v1/runs/abc",
        "/v1/runs/99999999999999999999",
        "/v1/runs?status=nope",
        "/v1/runs?kind=nope",
        "/v1/review/-1",
        "/v1/review/abc",
        "/v1/drafts/not-a-uuid",
        "/v1/documents/not-a-uuid",
        "/v1/explain?a=x&b=y",
        "/v1/explain",
        "/v1/constraints?record=x",
        "/v1/constraints?identity=x",
        "/v1/constraints?removed=maybe",
      ],
    ],
  ];
  for (const [status, paths] of reads) for (const path of paths) cases.push({ name: `GET ${path}`, status, path });
  cases.push({ name: "no token", status: 401, path: "/v1/info", token: null });
  cases.push({ name: "wrong token", status: 401, path: "/v1/info", token: "nope" });
  cases.push({ name: "token prefix", status: 401, path: "/v1/info", token: TOKEN.slice(0, -1) });
  cases.push({ name: "token longer", status: 401, path: "/v1/info", token: TOKEN + "x" });
  cases.push({
    name: "basic auth",
    status: 401,
    path: "/v1/info",
    token: null,
    headers: { authorization: `Basic ${btoa("a:" + TOKEN)}` },
  });
  cases.push({
    name: "lowercase bearer",
    status: 200,
    path: "/v1/info",
    token: null,
    headers: { authorization: `bearer ${TOKEN}` },
  });
  cases.push({ name: "64k header", status: 200, path: "/v1/info", headers: { "x-long": "h".repeat(64_000) } });
  for (const method of ["PUT", "DELETE", "PATCH", "OPTIONS"])
    cases.push({ name: `${method} /v1/info`, status: 405, method, path: "/v1/info" });
  cases.push({ name: "HEAD /v1/info", status: 200, method: "HEAD", path: "/v1/info" });
  cases.push({ name: "POST /v1/info", status: 405, method: "POST", path: "/v1/info", body: {} });
  cases.push({ name: "GET /v1/ingest", status: 405, path: "/v1/ingest" });

  // Changes.
  const actor = "user:stress";
  const nil = "00000000-0000-0000-0000-000000000000";
  const change = (name: string, status: number, path: string, body: unknown) =>
    cases.push({ name, status, method: "POST", path, body });
  change("commit: no ops", 409, "/v1/changesets", { actor, ops: [] });
  change("commit: unknown op", 400, "/v1/changesets", { actor, ops: [{ op: "explode" }] });
  change("commit: nil ids", 409, "/v1/changesets", { actor, ops: [{ op: "merge", from: nil, into: nil }] });
  change("commit: merge unknown", 409, "/v1/changesets", {
    actor,
    ops: [{ op: "merge", from: "11111111-1111-7111-8111-111111111111", into: "22222222-2222-7222-8222-222222222222" }],
  });
  change("commit: assign unknown record", 409, "/v1/changesets", {
    actor,
    ops: [{ op: "assign", record: nil, from: null, to: nil }],
  });
  change("commit: create identity unknown type", ANY, "/v1/changesets", {
    actor,
    ops: [{ op: "create_identity", identity: "33333333-3333-7333-8333-333333333333", entity_type: "person" }],
  });
  change("commit: create identity v4 uuid", 200, "/v1/changesets", {
    actor,
    ops: [{ op: "create_identity", identity: "33333333-3333-4333-8333-333333333333", entity_type: "company" }],
  });
  change("commit: create identity nil", 200, "/v1/changesets", {
    actor,
    ops: [{ op: "create_identity", identity: nil, entity_type: "company" }],
  });
  change("commit: create identity max uuid", 200, "/v1/changesets", {
    actor,
    ops: [{ op: "create_identity", identity: "ffffffff-ffff-ffff-ffff-ffffffffffff", entity_type: "company" }],
  });
  change("commit: actor invalid", 400, "/v1/changesets", {
    actor: "root",
    ops: [{ op: "create_identity", identity: "44444444-4444-7444-8444-444444444444", entity_type: "company" }],
  });
  change("commit: actor engine", ANY, "/v1/changesets", {
    actor: "engine",
    ops: [{ op: "create_identity", identity: "44444444-4444-7444-8444-444444444445", entity_type: "company" }],
  });
  change("commit: actor 100k", 200, "/v1/changesets", {
    actor: "user:" + "a".repeat(100_000),
    ops: [{ op: "create_identity", identity: "44444444-4444-7444-8444-444444444446", entity_type: "company" }],
  });
  change("commit: actor with newline", 200, "/v1/changesets", {
    actor: "user:a\nb",
    ops: [{ op: "create_identity", identity: "44444444-4444-7444-8444-444444444447", entity_type: "company" }],
  });
  change("commit: actor empty id", 400, "/v1/changesets", {
    actor: "user:",
    ops: [{ op: "create_identity", identity: "44444444-4444-7444-8444-444444444448", entity_type: "company" }],
  });
  change("commit: base negative", 400, "/v1/changesets", {
    actor,
    base: -1,
    ops: [{ op: "create_identity", identity: "55555555-5555-7555-8555-555555555555", entity_type: "company" }],
  });
  change("commit: base ahead", 409, "/v1/changesets", {
    actor,
    base: 1_000_000,
    ops: [{ op: "create_identity", identity: "55555555-5555-7555-8555-555555555556", entity_type: "company" }],
  });
  change("commit: base float", 400, "/v1/changesets", {
    actor,
    base: 0.5,
    ops: [{ op: "create_identity", identity: "55555555-5555-7555-8555-555555555557", entity_type: "company" }],
  });
  change("commit: reason 1MB", 200, "/v1/changesets", {
    actor,
    reason: long,
    ops: [{ op: "create_identity", identity: "66666666-6666-7666-8666-666666666666", entity_type: "company" }],
  });
  change("commit: reason NUL", 200, "/v1/changesets", {
    actor,
    reason: "a\u0000b",
    ops: [{ op: "create_identity", identity: "66666666-6666-7666-8666-666666666667", entity_type: "company" }],
  });
  change("commit: same identity twice", 409, "/v1/changesets", {
    actor,
    ops: [
      { op: "create_identity", identity: "66666666-6666-7666-8666-666666666668", entity_type: "company" },
      { op: "create_identity", identity: "66666666-6666-7666-8666-666666666668", entity_type: "company" },
    ],
  });
  change("commit: id given twice A", 200, "/v1/changesets", {
    actor,
    id: "77777777-7777-7777-8777-777777777777",
    ops: [{ op: "create_identity", identity: "66666666-6666-7666-8666-666666666669", entity_type: "company" }],
  });
  change("commit: id given twice B", 409, "/v1/changesets", {
    actor,
    id: "77777777-7777-7777-8777-777777777777",
    ops: [{ op: "create_identity", identity: "66666666-6666-7666-8666-66666666666a", entity_type: "company" }],
  });
  change("commit: run unknown", ANY, "/v1/changesets", {
    actor,
    run: 999999,
    ops: [{ op: "create_identity", identity: "66666666-6666-7666-8666-66666666666b", entity_type: "company" }],
  });
  change("commit: 50k creates", 200, "/v1/changesets", {
    actor,
    ops: Array.from({ length: 50_000 }, (_, i) => ({
      op: "create_identity",
      identity: `88888888-8888-7888-8888-${String(i).padStart(12, "0")}`,
      entity_type: "company",
    })),
  });
  change("undo: 0", 404, "/v1/changesets/0/undo", { actor });
  change("undo: huge", 404, "/v1/changesets/18446744073709551615/undo", { actor });
  change("undo: overflow", 400, "/v1/changesets/18446744073709551616/undo", { actor });
  change("undo: creates only", 409, "/v1/changesets/1/undo", { actor });
  // Positions past what the database counts in. The log has changesets by now, and none is after these.
  for (const after of ["9223372036854775808", "18446744073709551615"]) {
    cases.push({ name: `GET /v1/changesets?after=${after}`, status: 200, path: `/v1/changesets?after=${after}` });
  }
  change("split: unknown identity", 409, `/v1/identities/${nil}/split`, { actor, records: [nil] });
  change("split: no records", 409, `/v1/identities/66666666-6666-7666-8666-666666666666/split`, {
    actor,
    records: [],
  });
  change("mint: too many", 400, "/v1/ids", { identities: 10_001 });
  change("mint: negative", 400, "/v1/ids", { identities: -1 });
  change("mint: zero", 200, "/v1/ids", {});
  change("mint: float", 400, "/v1/ids", { identities: 1.5 });
  change("dismiss: unknown", 404, "/v1/review/999/dismiss", { actor });
  change("run: unknown type", 400, "/v1/runs", { entity_type: "person" });
  change("run: strategy name sql", 200, "/v1/runs", { entity_type: "company", strategy: { name: sql } });
  change("run: strategy name long", 200, "/v1/runs", {
    entity_type: "company",
    strategy: { name: "s".repeat(100_000) },
  });
  change("run: wrong version of project strategy", 400, "/v1/runs", {
    entity_type: "company",
    strategy: { name: "in_python", version: "nope" },
  });
  change("run: cancel unknown", 404, "/v1/runs/999999/cancel", {});
  change("judgements: unknown run", 404, "/v1/runs/999999/judgements", { judgements: [] });
  change("reconcile: unknown run", 404, "/v1/runs/999999/reconcile", {});
  change("draft approve: unknown", 404, `/v1/drafts/${nil}/approve`, {});
  change("documents: empty text", 400, "/v1/documents", { source: "news", documents: [{ key: "d1", text: "" }] });
  change("documents: key with slashes", 200, "/v1/documents", {
    source: "news",
    documents: [{ key: "../../etc/passwd", text: "Acme Ltd rose." }],
  });
  change("documents: NUL in text", 200, "/v1/documents", {
    source: "news",
    documents: [{ key: "d2", text: "Acme\u0000Ltd" }],
  });
  change("documents: 20MB text", 200, "/v1/documents", {
    source: "news",
    documents: [{ key: "d3", text: "Acme Ltd rose. ".repeat(1_400_000) }],
  });
  change("documents: duplicate keys", 200, "/v1/documents", {
    source: "news",
    documents: [
      { key: "d4", text: "one" },
      { key: "d4", text: "two" },
    ],
  });
  change("documents: no documents", 200, "/v1/documents", { source: "news", documents: [] });
  change("documents: key empty", 400, "/v1/documents", { source: "news", documents: [{ key: "", text: "x" }] });
  change("extract: unknown extractor", 400, "/v1/extractions", { extractor: "nope" });
  change("extract: sql extractor", 400, "/v1/extractions", { extractor: sql });
  return cases;
}

export async function hostile(log: (line: string) => void = () => {}): Promise<HostileResult> {
  const dir = scratch("hostile");
  copyCompanies(dir);
  writeFileSync(join(dir, "sources/scans.yaml"), "key: id\nevidence: ev\ncolumns: [id, name, ev]\n");
  appendFileSync(join(dir, "entities/company/entity.yaml"), "  scans:\n    name: name\n");
  const server = await serve({ project: dir, store: join(dir, "store.duckdb"), seed: 3 });
  const cases = all();
  const faults: string[] = [];
  try {
    for (const item of cases) {
      const headers: Record<string, string> = { ...(item.headers ?? {}) };
      if (item.token !== null && headers.authorization === undefined) {
        headers.authorization = `Bearer ${item.token ?? TOKEN}`;
      }
      let body: BodyInit | undefined;
      if (item.raw !== undefined) body = item.raw;
      else if (item.body !== undefined) {
        body = JSON.stringify(item.body);
        headers["content-type"] ??= "application/json";
      }
      const method = item.method ?? "GET";
      let status: number;
      let text: string;
      try {
        const response = await fetch(server.url + item.path, { method, headers, body });
        status = response.status;
        text = await response.text();
      } catch (error) {
        faults.push(`${item.name}: the request failed: ${(error as Error).message}`);
        if (!server.alive()) break;
        continue;
      }
      log(
        `${String(status).padEnd(4)} ${item.name.slice(0, 60).padEnd(60)} ${text.slice(0, 120).replaceAll("\n", " ")}`,
      );
      if (status >= 500) faults.push(`${item.name}: the server answered ${status}: ${text.slice(0, 300)}`);
      else if (item.status !== ANY && status !== item.status) {
        faults.push(`${item.name}: the server answered ${status}, not ${item.status}: ${text.slice(0, 300)}`);
      }
      if (status >= 400 && method !== "HEAD") {
        let code: unknown;
        try {
          code = (JSON.parse(text) as { error?: { code?: unknown; message?: unknown } }).error?.code;
        } catch {
          // Not JSON: there is no code.
        }
        if (typeof code !== "string")
          faults.push(`${item.name}: ${status} without the error body: ${text.slice(0, 300)}`);
      }
    }
    if (!server.alive()) {
      faults.push(`the server is gone: ${server.complaints()}`);
      return { cases: cases.length, faults };
    }

    // The server still does its work, and what it stored is sound.
    const run = await server.client.startRun({ entity_type: "company" });
    if (run?.status !== "succeeded") faults.push(`a match run afterwards ${run?.status}: ${run?.error ?? ""}`);
    const code = await server.stop();
    if (code !== 0) faults.push(`the server stopped with code ${code}: ${server.complaints()}`);
    try {
      verify(server.store);
    } catch (error) {
      faults.push((error as Error).message);
    }
    return { cases: cases.length, faults };
  } finally {
    if (server.alive()) await server.kill();
    if (process.env.SPOOL_STRESS_KEEP === undefined) rmSync(dir, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const result = await hostile((line) => console.log(`  ${line}`));
  console.log(`${result.cases} cases`);
  for (const fault of result.faults) console.log(`FAULT ${fault}`);
  process.exit(result.faults.length === 0 ? 0 : 1);
}
