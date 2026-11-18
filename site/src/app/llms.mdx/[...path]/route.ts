import { notFound } from "next/navigation";

import { markdownPath, pageMarkdown } from "@/lib/markdown";
import { source } from "@/lib/source";

export const revalidate = false;
export const dynamicParams = false;

type Context = { params: Promise<{ path: string[] }> };

/**
 * A page as Markdown. `/docs/getting-started.md` is rewritten to this route
 * in `next.config.mjs`, and so is a request for the page itself that asks for
 * `text/markdown`, in `proxy.ts`.
 */
export async function GET(_request: Request, { params }: Context) {
  const { path } = await params;
  const wanted = `/${path.join("/")}`;
  const page = source.getPages().find((candidate) => markdownPath(candidate.url) === wanted);
  if (!page) notFound();

  return new Response(await pageMarkdown(page), {
    headers: {
      "Content-Type": "text/markdown; charset=utf-8",
      Vary: "Accept",
    },
  });
}

export function generateStaticParams() {
  return source.getPages().map((page) => ({
    path: markdownPath(page.url).split("/").filter(Boolean),
  }));
}
