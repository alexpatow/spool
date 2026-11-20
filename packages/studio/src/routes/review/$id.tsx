import { createFileRoute } from "@tanstack/react-router";

import { ReviewItem } from "../../screens/review.tsx";

export const Route = createFileRoute("/review/$id")({ component: ReviewItem });
