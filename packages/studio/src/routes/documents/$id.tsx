import { createFileRoute } from "@tanstack/react-router";

import { filters, number } from "../../address.ts";
import type { Raw } from "../../address.ts";
import { Document } from "../../screens/documents.tsx";

export const Route = createFileRoute("/documents/$id")({
  // `at` is where in the text a mention starts: the one the reader came from.
  validateSearch: (raw: Raw) => filters(raw, { at: number }),
  component: Document,
});
