import { createFileRoute } from "@tanstack/react-router";

import { filters, flag, sorted, text } from "../../address.ts";
import type { Raw } from "../../address.ts";
import { RecordsPage } from "../../screens/records.tsx";

export const Route = createFileRoute("/records/")({
  validateSearch: (raw: Raw) =>
    filters(raw, { q: text, type: text, source: text, key: text, document: text, unassigned: flag, ...sorted }),
  component: RecordsPage,
});
