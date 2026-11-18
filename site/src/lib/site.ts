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
  /** Where the site is published. `SITE_URL`, when it is set, takes its place. */
  url: "https://spool.alexpatow.com",
} as const;

/**
 * The origin every absolute address is built on: canonical links, the
 * addresses a link preview reads, the sitemap, the `url` in a page's
 * Markdown, and the links in `llms.txt`.
 */
export function siteOrigin(): string {
  return (process.env["SITE_URL"] || site.url).replace(/\/+$/, "");
}

export function absoluteUrl(path: string): string {
  return `${siteOrigin()}${path}`;
}
