import type { MetadataRoute } from "next";

import { navPages } from "@/lib/nav";
import { absoluteUrl } from "@/lib/site";

export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: absoluteUrl("/") },
    ...navPages().map((page) => ({ url: absoluteUrl(page.url) })),
  ];
}
