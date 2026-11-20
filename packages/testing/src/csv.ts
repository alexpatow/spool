// Reads the CSV files of the repository's scenarios as the engine does.

import { readFileSync } from "node:fs";

/**
 * Reads a CSV file with a header row into one object per row. Every value is
 * text, and an empty field is left out, which is how the engine reads CSV.
 */
export function readCsv(path: string): Array<Record<string, string>> {
  const text = readFileSync(path, "utf8");
  const records: string[][] = [];
  let record: string[] = [];
  let field = "";
  let quoted = false;
  for (let index = 0; index < text.length; index++) {
    const char = text[index]!;
    if (quoted) {
      if (char === '"' && text[index + 1] === '"') {
        field += '"';
        index++;
      } else if (char === '"') {
        quoted = false;
      } else {
        field += char;
      }
    } else if (char === '"') {
      quoted = true;
    } else if (char === ",") {
      record.push(field);
      field = "";
    } else if (char === "\n" || char === "\r") {
      if (char === "\r" && text[index + 1] === "\n") index++;
      record.push(field);
      records.push(record);
      record = [];
      field = "";
    } else {
      field += char;
    }
  }
  if (field !== "" || record.length > 0) {
    record.push(field);
    records.push(record);
  }

  const [header, ...rows] = records;
  if (header === undefined) return [];
  return rows
    .filter((row) => row.some((value) => value !== ""))
    .map((row) => {
      const object: Record<string, string> = {};
      header.forEach((name, column) => {
        const value = row[column];
        if (value !== undefined && value !== "") object[name] = value;
      });
      return object;
    });
}
