import { createFileRoute } from "@tanstack/react-router";

import { filters, flag, sorted, text } from "../../address.ts";
import type { Raw } from "../../address.ts";
import { Documents } from "../../screens/documents.tsx";

export const Route = createFileRoute("/documents/")({
  validateSearch: (raw: Raw) => filters(raw, { source: text, key: text, latest: flag, ...sorted }),
  component: Documents,
});
