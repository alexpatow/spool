/** What the site says about itself, in one place. */
export const site = {
  name: "Spool",
  summary:
    "Spool is a framework for entity resolution with persistent identity. It decides which records from your sources are the same real-world thing, and gives each one an ID that stays the same as data arrives.",
  description:
    "Entity resolution with persistent identity: declare sources and rules as files, match records, and keep stable IDs as data changes.",
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
