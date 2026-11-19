/** What the site says about itself, in one place. */
const summary =
  "Give real-world things persistent IDs. Spool extracts records from text and matches them with structured data, keeping identities consistent as new information arrives.";

export const site = {
  name: "Spool",
  /** The front page's headline. */
  headline: "One identity. Every source.",
  /** The sentence under it, and the description a search result shows. */
  summary,
  title: "Spool: one identity across every source",
  description: summary,
  // What `spool --version` prints. The samples check compares the two on the
  // Getting started page.
  version: "0.3.0",
  licence: "MIT",
  repository: "https://github.com/alexpatow/spool",
  /** The X account a shared page names as its site and its author. Spool has none of its own. */
  x: "@alexpatow",
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
