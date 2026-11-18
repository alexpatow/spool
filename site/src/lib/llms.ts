import { pageBody, markdownPath } from "@/lib/markdown";
import { navGroups } from "@/lib/nav";
import { absoluteUrl, site } from "@/lib/site";
import { source } from "@/lib/source";

/**
 * `/llms.txt`: what Spool is, how to read these docs as an agent, and one
 * line per page, grouped as the sidebar groups them.
 */
export function llmsIndex(): string {
  const lines = [
    `# ${site.name}`,
    "",
    `> ${site.summary}`,
    "",
    "These are the Spool docs for agents and other programs. Every page is also plain Markdown: add `.md` to its address, or ask for it with `Accept: text/markdown`. Each line below links to a page's Markdown and says what the page covers.",
    "",
    `- All pages in one file: ${absoluteUrl("/llms-full.txt")}`,
    `- Source code: ${site.repository}`,
    "",
    "Spool is early. Nothing is published yet, so it is built from source, and the Limits page says what it does not do.",
  ];

  for (const group of navGroups()) {
    lines.push("", `## ${group.title}`, "");
    for (const page of group.pages) {
      const description = page.description ? `: ${page.description}` : "";
      lines.push(`- [${page.title}](${absoluteUrl(markdownPath(page.url))})${description}`);
    }
  }

  return `${lines.join("\n")}\n`;
}

/** `/llms-full.txt`: every page, in reading order, in one file. */
export async function llmsFull(): Promise<string> {
  const parts = [`# ${site.name} documentation`, "", `> ${site.summary}`, ""];

  for (const group of navGroups()) {
    for (const entry of group.pages) {
      const page = source.getPages().find((candidate) => candidate.url === entry.url);
      if (!page) continue;
      parts.push("---", "", `Source: ${absoluteUrl(markdownPath(page.url))}`, "", await pageBody(page));
    }
  }

  return parts.join("\n");
}
