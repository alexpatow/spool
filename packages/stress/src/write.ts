// Writes a generated set as a project that `spool bench` runs:
//
//   bun src/write.ts --companies 50000 --out /tmp/spool-stress/50k
//
// The directory gets a copy of the company fixture's entity type, the
// records as CSV files, their truth, and a scenario of one step per batch.

import { cpSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { crmCsv, generate, records, registryCsv, truthCsv } from "./generate.ts";
import type { Generated } from "./generate.ts";

export const repository = resolve(import.meta.dir, "../../..");
export const companiesProject = join(repository, "benchmarks/identity/companies-small");

/** Writes the project and returns how many records it holds. */
export function writeProject(dir: string, generated: Generated): number {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  cpSync(join(companiesProject, "entities"), join(dir, "entities"), { recursive: true });
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
        'key = "id"',
        "",
        "[[step.ingest]]",
        `file = "batch-${number}-registry.csv"`,
        'source = "registry"',
        'entity_type = "company"',
        'key = "number"',
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
