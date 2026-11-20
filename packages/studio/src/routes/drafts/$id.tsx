import { createFileRoute } from "@tanstack/react-router";

import { Draft } from "../../screens/drafts.tsx";

export const Route = createFileRoute("/drafts/$id")({ component: Draft });
