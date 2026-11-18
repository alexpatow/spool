/** What the site says about itself, in one place. */
export const site = {
  name: "Spool",
  /** The front page's headline. */
  headline: "Extract, resolve and track entities.",
  /** The sentence under it, and the description a search result shows. */
  summary:
    "Spool pulls records out of text and rows, decides which are the same real-world thing, and gives each thing an ID that stays the same as more arrive.",
  title: "Spool: extract, resolve and track entities",
  description:
    "Spool pulls records out of text and rows, decides which are the same real-world thing, and gives each thing an ID that stays the same as more arrive.",
  // What `spool --version` prints. The samples check compares the two on the
  // Getting started page.
  version: "0.1.0",
  licence: "MIT",
  repository: "https://github.com/alexpatow/spool",
} as const;

/**
 * The origin every absolute address is built on: the sitemap, the `url` in a
 * page's Markdown, and the links in `llms.txt`.
 */
export function siteOrigin(): string {
  const explicit = process.env["SITE_URL"];
  if (explicit) return explicit.replace(/\/+$/, "");
  const production = process.env["VERCEL_PROJECT_PRODUCTION_URL"];
  if (production) return `https://${production}`;
  return `http://localhost:${process.env["PORT"] ?? "3000"}`;
}

export function absoluteUrl(path: string): string {
  return `${siteOrigin()}${path}`;
}
