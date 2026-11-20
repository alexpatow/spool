import { createFileRoute } from "@tanstack/react-router";

import { Changeset } from "../../screens/log.tsx";

export const Route = createFileRoute("/log/$id")({ component: Changeset });
