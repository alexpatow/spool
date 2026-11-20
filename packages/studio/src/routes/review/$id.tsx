import { createFileRoute } from "@tanstack/react-router";

import { ReviewItem } from "../../screens/review/case.tsx";

export const Route = createFileRoute("/review/$id")({ component: ReviewItem });
