import { notFound } from "next/navigation";

import { navGroups } from "@/lib/nav";
import { source } from "@/lib/source";
import { card } from "@/og/card";

export const revalidate = false;
export const dynamicParams = false;

type Context = { params: Promise<{ slug: string[] }> };

/** A docs page's card, drawn when the site is built: `/og/docs/concepts/matching/image.png`. */
export async function GET(_request: Request, { params }: Context) {
  const { slug } = await params;
  const page = source.getPage(slug.slice(0, -1));
  if (!page) notFound();
  const group = navGroups().find((candidate) => candidate.pages.some((entry) => entry.url === page.url));

  return card({
    section: group ? `Docs / ${group.title}` : "Docs",
    title: page.data.title,
    description: page.data.description ?? "",
    path: page.url,
  });
}

export function generateStaticParams() {
  return source.getPages().map((page) => ({ slug: [...page.slugs, "image.png"] }));
}
