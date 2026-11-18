/**
 * The sample project of Getting started, as it stands at the end of that page:
 * the nine rows of `samples/crm-1.csv` and `samples/crm-2.csv`, the note
 * `samples/notes-1.txt`, and the identities a match gave them. The front page
 * draws its first screen from this, so it shows what the commands print.
 */

export const note = {
  file: "samples/notes-1.txt",
  key: "notes-1.txt",
  text:
    "Call with Dana Reyes on 12 November. She has moved to Acorn Tools Ltd and runs\n" +
    "purchasing there. They now ship through Larkfield Logistics Ltd, and she asked\n" +
    "whether we work with Brightwater Foods Ltd, their new supplier.",
} as const;

export interface Identity {
  id: string;
  name: string;
  /** Said beside the name where two identities share one. */
  detail?: string;
}

/** In the order `spool show identities` lists them after the three matches. */
export const identities = [
  { id: "01a1ef8e-5a8d-76d4-acb6-2bb5ad4d4ada", name: "Acorn Tools" },
  { id: "01a1ef8e-5a8d-76d4-acb6-2bb802d5ca25", name: "Larkfield Logistics" },
  { id: "01a1ef8e-5a8d-76d4-acb6-2bb793e9d0ae", name: "Halden Group", detail: "GB" },
  { id: "01a1ef8e-5a8d-76d4-acb6-2bb65a02262a", name: "Halden Group", detail: "US" },
  { id: "01a1ef8e-5f06-7397-9a0c-445aaacfd3fe", name: "Kestrel Analytics" },
  { id: "01a1efb4-93c3-7745-8408-dcbf0c81e6f3", name: "Brightwater Foods" },
] as const satisfies readonly Identity[];

export interface SampleRecord {
  source: "crm" | "notes";
  key: string;
  name: string;
  /** A row's country. A record from text has none. */
  country?: string;
  /** The place of its identity in `identities`. */
  identity: number;
  /** The place in `log` of the match that put it there. */
  match: number;
}

/** In the order `spool show records` lists them: by source, then by key. */
export const records: readonly SampleRecord[] = [
  { source: "crm", key: "c-1", name: "Acorn Tools", country: "GB", identity: 0, match: 0 },
  { source: "crm", key: "c-2", name: "ACORN TOOLS", country: "GB", identity: 0, match: 0 },
  { source: "crm", key: "c-3", name: "Acorn Tools Ltd", country: "GB", identity: 0, match: 0 },
  { source: "crm", key: "c-4", name: "Larkfield Logistics", country: "GB", identity: 1, match: 0 },
  { source: "crm", key: "c-5", name: "Halden Group", country: "GB", identity: 2, match: 0 },
  { source: "crm", key: "c-6", name: "Halden Group", country: "US", identity: 3, match: 0 },
  { source: "crm", key: "c-7", name: "Larkfeild Logistics", country: "GB", identity: 1, match: 1 },
  { source: "crm", key: "c-8", name: "Kestrel Analytics", country: "US", identity: 4, match: 1 },
  { source: "crm", key: "c-9", name: "Acorn Tool", country: "GB", identity: 0, match: 1 },
  ...mentions(["Acorn Tools Ltd", "Larkfield Logistics Ltd", "Brightwater Foods Ltd"], [0, 1, 5]),
];

/** A record for each name the sample extractor finds in the note, keyed by where it is. */
function mentions(names: string[], placed: number[]): SampleRecord[] {
  return names.map((name, index) => {
    const start = note.text.indexOf(name);
    return {
      source: "notes",
      key: `${note.key}[${start}-${start + name.length}]`,
      name,
      identity: placed[index] ?? 0,
      match: 2,
    };
  });
}

/** What each of the three matches wrote to the changeset log, oldest first. */
export const log = [
  { after: "crm-1.csv", did: "4 identities created, 6 records placed" },
  { after: "crm-2.csv", did: "1 identity created, 3 records placed" },
  { after: "notes-1.txt", did: "1 identity created, 3 records placed" },
] as const;
