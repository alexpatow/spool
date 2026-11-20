// Fills a server with the repository's two small scenarios, so that every
// screen of the studio has something to show. The tests and `bun run demo`
// both use it.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { copyProject as copyProjectTo, readCsv, repository } from "spool-testing";

import type { RecordEntry, SpoolClient } from "../src/api.ts";

export const companies = join(repository, "benchmarks/identity/companies-small");
export const news = join(repository, "benchmarks/extraction/company-news");

/** Copies what a project is made of, so that building it leaves the original alone. */
export function copyProject(from: string): string {
  return copyProjectTo(from, "spool-studio");
}

async function load(client: SpoolClient, file: string, source: string): Promise<void> {
  const rows = readCsv(join(companies, file));
  const run = await client.load({ source, entity_type: "company", rows });
  if (run.status !== "succeeded") throw new Error(`loading ${file} ${run.status}: ${run.error ?? ""}`);
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
  await load(client, "batch-1-crm.csv", "crm");
  await load(client, "batch-1-registry.csv", "registry");
  await client.match({ entity_type: "company" });

  const atlas = await client.commit({
    reason: "same name, taken for one company",
    ops: [
      { op: "merge", from: await identityOf(client, "crm", "c-109"), into: await identityOf(client, "crm", "c-108") },
    ],
  });
  await client.commit({
    reason: "same name, taken for one company",
    ops: [
      {
        op: "merge",
        from: await identityOf(client, "registry", "X0000010"),
        into: await identityOf(client, "registry", "X0000009"),
      },
    ],
  });
  await client.undo(atlas.seq, { reason: "they are two companies after all" });
  await client.split(await identityOf(client, "registry", "X0000010"), {
    reason: "the Bristol company is another company",
    records: [(await row(client, "registry", "X0000010")).id],
  });
  await client.commit({
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

  await load(client, "batch-2-crm.csv", "crm");
  await client.match({ entity_type: "company" });
  await load(client, "batch-2-registry.csv", "registry");
  await client.match({ entity_type: "company", commit: false });
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
    await client.match({ entity_type: "company" });
  }
}
