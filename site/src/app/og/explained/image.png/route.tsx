import { explainer } from "@/lib/explainer";
import { card } from "@/og/card";

export const revalidate = false;

/** The explainer's card: `/og/explained/image.png`. */
export function GET() {
  return card({
    section: "Explained",
    title: explainer.title,
    description: explainer.description,
    path: explainer.path,
  });
}
