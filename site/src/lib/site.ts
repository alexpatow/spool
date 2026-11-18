/** What the site says about itself, in one place. */
export const site = {
  name: "Spool",
  summary:
    "Spool decides which records are the same real-world thing, and gives each thing an ID that stays the same as more records arrive.",
  description:
    "Entity resolution with persistent identity. Sources and rules are files, records are matched in runs, and identities keep their IDs as data changes.",
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
