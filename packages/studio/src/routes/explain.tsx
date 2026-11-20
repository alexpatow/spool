import { createFileRoute } from "@tanstack/react-router";

import { filters, text } from "../address.ts";
import type { Raw } from "../address.ts";
import { Explain } from "../screens/explain.tsx";

export const Route = createFileRoute("/explain")({
  // The two records of the pair, each by its ID.
  validateSearch: (raw: Raw) => filters(raw, { a: text, b: text }),
  component: Explain,
});
