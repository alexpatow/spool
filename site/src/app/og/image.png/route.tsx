import { site } from "@/lib/site";
import { card } from "@/og/card";

export const revalidate = false;

/** The front page's card, and the card of any page without one of its own. */
export function GET() {
  return card({
    section: `Version ${site.version}, ${site.licence} licence`,
    title: site.headline,
    description: site.summary,
    path: "/",
  });
}
