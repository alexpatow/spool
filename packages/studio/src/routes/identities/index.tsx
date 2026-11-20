import { createFileRoute } from "@tanstack/react-router";

import { filters, sorted, text } from "../../address.ts";
import type { Raw } from "../../address.ts";
import { Identities } from "../../screens/identities.tsx";

export const Route = createFileRoute("/identities/")({
  validateSearch: (raw: Raw) => filters(raw, { q: text, type: text, ...sorted }),
  component: Identities,
});
