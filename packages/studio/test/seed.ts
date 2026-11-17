// Fills a server with the repository's two small scenarios, so that every
// screen of the studio has something to show. The tests and `bun run demo`
// both use it.

import { cpSync, existsSync, mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { readCsv, repository } from "../../client/test/server.ts";
import type { RecordEntry, SpoolClient } from "../src/api.ts";

export const companies = join(repository, "benchmarks/identity/companies-small");
export const news = join(repository, "benchmarks/extraction/company-news");

/** Who made the corrections of the company scenario. */
export const REVIEWER = "user:studio-demo";

/** Copies what a project is made of, so that building it leaves the original alone. */
export function copyProject(from: string): string {
  const to = mkdtempSync(join(tmpdir(), "spool-studio-"));
  for (const part of ["entities", "extractors"]) {
    if (existsSync(join(from, part))) cpSync(join(from, part), join(to, part), { recursive: true });
  }
  return to;
}

async function ingest(client: SpoolClient, file: string, source: string, key: string): Promise<void> {
  const rows = readCsv(join(companies, file));
  const run = await client.ingest({ source, entity_type: "company", key, rows });
  if (run.status !== "succeeded") throw new Error(`ingesting ${file} ${run.status}: ${run.error ?? ""}`);
}

export async function row(client: SpoolClient, source: string, key: string): Promise<RecordEntry> {
  const { records } = await client.records({ source, key });
  const latest = records.find((record) => !record.superseded_by);
  if (latest === undefined) throw new Error(`there is no ${source}:${key}`);
  return latest;
}

async function identityOf(client: SpoolClient, source: string, key: string): Promise<string> {
  const { identity } = await row(client, source, key);
  if (!identity) throw new Error(`${source}:${key} has no identity`);
  return identity;
}

/**
 * Company records from a CRM and a registry, in two batches, with a reviewer
 * at work between them: a merge that is undone, a merge that is split, a
 * match only a person knows, a record left for review, and a draft that
 * waits for a decision.
 */
export async function seedCompanies(client: SpoolClient): Promise<void> {
  await ingest(client, "batch-1-crm.csv", "crm", "id");
  await ingest(client, "batch-1-registry.csv", "registry", "number");
  await client.startRun({ entity_type: "company" });

  const atlas = await client.commit({
    actor: REVIEWER,
    reason: "same name, taken for one company",
    ops: [
      { op: "merge", from: await identityOf(client, "crm", "c-109"), into: await identityOf(client, "crm", "c-108") },
    ],
  });
  await client.commit({
    actor: REVIEWER,
    reason: "same name, taken for one company",
    ops: [
      {
        op: "merge",
        from: await identityOf(client, "registry", "X0000010"),
        into: await identityOf(client, "registry", "X0000009"),
      },
    ],
  });
  await client.undo(atlas.seq, { actor: REVIEWER, reason: "they are two companies after all" });
  await client.split(await identityOf(client, "registry", "X0000010"), {
    actor: REVIEWER,
    reason: "the Bristol company is another company",
    records: [(await row(client, "registry", "X0000010")).id],
  });
  await client.commit({
    actor: REVIEWER,
    reason: "Orchard is Orchard Foods",
    ops: [
      {
        op: "merge",
        from: await identityOf(client, "crm", "c-111"),
        into: await identityOf(client, "registry", "X0000011"),
      },
      {
        op: "constrain",
        kind: "must_link",
        a: (await row(client, "crm", "c-111")).id,
        b: (await row(client, "registry", "X0000011")).id,
      },
    ],
  });

  await ingest(client, "batch-2-crm.csv", "crm", "id");
  await client.startRun({ entity_type: "company" });
  await ingest(client, "batch-2-registry.csv", "registry", "number");
  await client.startRun({ entity_type: "company", commit: false });
}

/**
 * News items in two batches, read by the extractor that is written in
 * TypeScript, and the companies they mention resolved.
 */
export async function seedNews(client: SpoolClient): Promise<void> {
  for (const batch of ["batch-1", "batch-2"]) {
    const dir = join(news, "documents", batch);
    const documents = readdirSync(dir)
      .sort()
      .map((file) => ({ key: file, text: readFileSync(join(dir, file), "utf8") }));
    await client.addDocuments("news", documents);
    const run = await client.extract("suffix_ts");
    if (run.status !== "succeeded") throw new Error(`the extractor ${run.status}: ${run.error ?? ""}`);
    await client.startRun({ entity_type: "company" });
  }
}
