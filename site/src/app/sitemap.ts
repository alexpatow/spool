import type { MetadataRoute } from "next";

import { explainer } from "@/lib/explainer";
import { navPages } from "@/lib/nav";
import { absoluteUrl } from "@/lib/site";

export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: absoluteUrl("/") },
    { url: absoluteUrl(explainer.path) },
    ...navPages().map((page) => ({ url: absoluteUrl(page.url) })),
  ];
}
