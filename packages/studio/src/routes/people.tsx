import { createFileRoute } from "@tanstack/react-router";

import { People } from "../screens/people.tsx";

export const Route = createFileRoute("/people")({ component: People });
