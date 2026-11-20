import { createFileRoute } from "@tanstack/react-router";

import { RecordPage } from "../../screens/records.tsx";

export const Route = createFileRoute("/records/$id")({ component: RecordPage });
