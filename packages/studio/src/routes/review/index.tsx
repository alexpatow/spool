import { createFileRoute } from "@tanstack/react-router";

import { filters, text } from "../../address.ts";
import type { Raw } from "../../address.ts";
import { ReviewItems } from "../../screens/review.tsx";

export const Route = createFileRoute("/review/")({
  // `sort` is one of the orders the screen offers. The items are read whole and put in order in the page.
  validateSearch: (raw: Raw) => filters(raw, { status: text, type: text, sort: text }),
  component: ReviewItems,
});
