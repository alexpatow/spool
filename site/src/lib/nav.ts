import type { Node } from "fumadocs-core/page-tree";

import { source } from "@/lib/source";

export interface NavPage {
  title: string;
  url: string;
  description: string;
}

export interface NavGroup {
  title: string;
  pages: NavPage[];
}

/**
 * The pages in reading order, grouped as `content/docs/meta.json` groups
 * them. The sidebar, the previous and next links, and the sections of
 * `llms.txt` are all this one list.
 */
export function navGroups(): NavGroup[] {
  const descriptions = new Map(source.getPages().map((page) => [page.url, page.data.description ?? ""]));
  const groups: NavGroup[] = [];
  let current: NavGroup | undefined;

  const add = (node: Node) => {
    if (node.type === "separator") {
      current = { title: String(node.name ?? ""), pages: [] };
      groups.push(current);
      return;
    }
    if (node.type === "folder") {
      if (node.index) add(node.index);
      node.children.forEach(add);
      return;
    }
    if (!current) {
      current = { title: "Docs", pages: [] };
      groups.push(current);
    }
    current.pages.push({
      title: String(node.name),
      url: node.url,
      description: descriptions.get(node.url) ?? "",
    });
  };

  source.getPageTree().children.forEach(add);
  return groups.filter((group) => group.pages.length > 0);
}

export function navPages(): NavPage[] {
  return navGroups().flatMap((group) => group.pages);
}

export function neighbours(url: string): { previous?: NavPage; next?: NavPage } {
  const pages = navPages();
  const index = pages.findIndex((page) => page.url === url);
  if (index === -1) return {};
  const previous = pages[index - 1];
  const next = pages[index + 1];
  return { ...(previous && { previous }), ...(next && { next }) };
}
