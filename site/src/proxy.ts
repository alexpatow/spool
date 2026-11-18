import { isMarkdownPreferred, rewritePath } from "fumadocs-core/negotiation";
import { type NextRequest, NextResponse } from "next/server";

const { rewrite } = rewritePath("/docs{/*path}", "/llms.mdx/docs{/*path}.md");

/**
 * A request for a docs page that prefers Markdown (`Accept: text/markdown`)
 * is answered with the page's Markdown, the same text its `.md` address
 * serves. Either way the answer says it varied by `Accept`, so a cache keeps
 * the two apart.
 */
export default function proxy(request: NextRequest) {
  if (isMarkdownPreferred(request)) {
    const target = rewrite(request.nextUrl.pathname);
    if (target) return NextResponse.rewrite(new URL(target, request.nextUrl));
  }

  const response = NextResponse.next();
  response.headers.append("Vary", "Accept");
  return response;
}

export const config = {
  // Pages only: the Markdown and text addresses are already what was asked for.
  matcher: ["/docs", "/docs/((?!.*\\.(?:md|txt)$).*)"],
};
