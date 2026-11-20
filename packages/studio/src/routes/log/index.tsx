import { createFileRoute } from "@tanstack/react-router";

import { filters, sorted } from "../../address.ts";
import type { Raw } from "../../address.ts";
import { Log } from "../../screens/log.tsx";

export const Route = createFileRoute("/log/")({
  validateSearch: (raw: Raw) => filters(raw, sorted),
  component: Log,
});
