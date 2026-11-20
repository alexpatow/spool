// Writes a generated set as a project that `spool bench` runs:
//
//   bun src/write.ts --companies 50000 --out /tmp/spool-stress/50k
//
// The directory gets a copy of the company fixture's entity type, the
// records as CSV files, their truth, and a scenario of one step per batch.

import { cpSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { repository } from "spool-testing";

import { crmCsv, generate, records, registryCsv, truthCsv } from "./generate.ts";
import type { Generated } from "./generate.ts";

export const companiesProject = join(repository, "benchmarks/identity/companies-small");

/** Writes the project and returns how many records it holds. */
/** Copies the company fixture's sources, transforms and entity type into `dir`. */
export function copyCompanies(dir: string): void {
  for (const part of ["sources", "transforms", "entities"]) {
    cpSync(join(companiesProject, part), join(dir, part), { recursive: true });
  }
}

export function writeProject(dir: string, generated: Generated): number {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  copyCompanies(dir);
  const steps: string[] = [];
  generated.batches.forEach((batch, index) => {
    const number = index + 1;
    writeFileSync(join(dir, `batch-${number}-crm.csv`), crmCsv(batch.crm));
    writeFileSync(join(dir, `batch-${number}-registry.csv`), registryCsv(batch.registry));
    steps.push(
      [
        "[[step]]",
        `name = "batch ${number}"`,
        "",
        "[[step.ingest]]",
        `file = "batch-${number}-crm.csv"`,
        'source = "crm"',
        'entity_type = "company"',
        "",
        "[[step.ingest]]",
        `file = "batch-${number}-registry.csv"`,
        'source = "registry"',
        'entity_type = "company"',
        "",
      ].join("\n"),
    );
  });
  writeFileSync(join(dir, "truth.csv"), truthCsv(generated.truth));
  writeFileSync(
    join(dir, "scenario.toml"),
    ['project = "."', "", "[truth]", 'file = "truth.csv"', "", ...steps].join("\n"),
  );
  return records(generated);
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const option = (name: string, otherwise: string) =>
    (args.includes(name) && args[args.indexOf(name) + 1]) || otherwise;
  const companies = Number(option("--companies", "10000"));
  const seed = Number(option("--seed", "1"));
  const batches = Number(option("--batches", "2"));
  const out = option("--out", `/tmp/spool-stress/${companies}`);
  const count = writeProject(out, generate({ companies, seed, batches }));
  console.log(`${count} records of ${companies} companies in ${out}`);
}
