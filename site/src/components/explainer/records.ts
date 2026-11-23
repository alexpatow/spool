// The sample's records as the explainer's scenes show them.

import { records, type SampleRecord } from "@/lib/sample";

/** The rows of the CRM's exports. */
export const rows = records.filter((record) => record.source === "crm");

/** The records the extractor made of what it found in the note. */
export const mentions = records.filter((record) => record.source === "notes");

/** How `spool show records` names a record: its source and its key. */
export function qualifiedKey(record: SampleRecord): string {
  return `${record.source}:${record.key}`;
}
