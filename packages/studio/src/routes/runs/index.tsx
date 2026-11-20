import { createFileRoute } from "@tanstack/react-router";

import { filters, number, sorted, text } from "../../address.ts";
import type { Raw } from "../../address.ts";
import { Runs } from "../../screens/runs.tsx";

export const Route = createFileRoute("/runs/")({
  // `limit` is how many runs are listed: the list is not read in pages.
  validateSearch: (raw: Raw) => filters(raw, { kind: text, status: text, type: text, limit: number, ...sorted }),
  component: Runs,
});
