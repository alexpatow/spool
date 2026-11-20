import { createFileRoute } from "@tanstack/react-router";

import { Tokens } from "../screens/tokens.tsx";

export const Route = createFileRoute("/tokens")({ component: Tokens });
