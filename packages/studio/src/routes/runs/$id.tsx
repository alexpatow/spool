import { createFileRoute } from "@tanstack/react-router";

import { Run } from "../../screens/runs.tsx";

export const Route = createFileRoute("/runs/$id")({ component: Run });
