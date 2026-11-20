import { createFileRoute } from "@tanstack/react-router";

import { Overview } from "../screens/overview.tsx";

export const Route = createFileRoute("/")({ component: Overview });
