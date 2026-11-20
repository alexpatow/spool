import { createFileRoute } from "@tanstack/react-router";

import { Identity } from "../../screens/identities.tsx";

export const Route = createFileRoute("/identities/$id")({ component: Identity });
