import { createFileRoute } from "@tanstack/react-router";

import { filters, text } from "../../address.ts";
import type { Raw } from "../../address.ts";
import { Drafts } from "../../screens/drafts.tsx";

export const Route = createFileRoute("/drafts/")({
  validateSearch: (raw: Raw) => filters(raw, { status: text, type: text }),
  component: Drafts,
});
