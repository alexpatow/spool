import type { InferPageType } from "fumadocs-core/source";

import { absoluteUrl } from "@/lib/site";
import type { source } from "@/lib/source";

type Page = InferPageType<typeof source>;

/** The address of a page's Markdown: the page's own address plus `.md`. */
export function markdownPath(url: string): string {
  return `${url}.md`;
}

function yamlString(value: string): string {
  return JSON.stringify(value);
}

/**
 * Links between pages are written as site paths (`/docs/...`). In the
 * Markdown they become absolute addresses of the other page's Markdown, so
 * an agent can follow them from wherever it has the text. Code is left alone.
 */
function absoluteLinks(markdown: string): string {
  let fenced = false;
  return markdown
    .split("\n")
    .map((line) => {
      if (/^\s*(```|~~~)/.test(line)) {
        fenced = !fenced;
        return line;
      }
      if (fenced) return line;
      return line.replace(
        /\]\((\/docs(?:\/[^)#\s]*)?)(#[^)\s]*)?\)/g,
        (_match, path: string, hash: string | undefined) =>
          `](${absoluteUrl(markdownPath(path.replace(/\/$/, "")))}${hash ?? ""})`,
      );
    })
    .join("\n");
}

/** A page's title, description and text, with no front matter. */
export async function pageBody(page: Page): Promise<string> {
  const text = await page.data.getText("processed");
  const description = page.data.description ? `${page.data.description}\n\n` : "";
  return `# ${page.data.title}\n\n${description}${absoluteLinks(text).trim()}\n`;
}

/** What `/docs/<page>.md` serves. */
export async function pageMarkdown(page: Page): Promise<string> {
  const frontMatter = [
    "---",
    `title: ${yamlString(page.data.title)}`,
    `description: ${yamlString(page.data.description ?? "")}`,
    `url: ${absoluteUrl(page.url)}`,
    `docs_index: ${absoluteUrl("/llms.txt")}`,
    "---",
  ].join("\n");
  return `${frontMatter}\n\n${await pageBody(page)}`;
}
