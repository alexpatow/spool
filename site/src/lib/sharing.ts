import type { Metadata } from "next";

import { site } from "@/lib/site";

/** The size of every card, which is what X, LinkedIn, Slack and Discord all crop to. */
export const cardSize = { width: 1200, height: 630 } as const;

/** Where the card of the front page is, and of every other page that has none of its own. */
export const siteCardPath = "/og/image.png";

/** Where the card of a docs page is: `/docs/concepts/matching` has `/og/docs/concepts/matching/image.png`. */
export function docsCardPath(url: string): string {
  return `/og${url}/image.png`;
}

interface Shared {
  /** The title a link preview shows, in full: no template is added to it. */
  title: string;
  description: string;
  /** The page's own address, relative to the site. It is also the canonical one. */
  path: string;
  /** The card's address, relative to the site. */
  image: string;
  /** What the card shows, for a reader who cannot see it. */
  imageAlt: string;
  type: "website" | "article";
}

/**
 * What a link preview reads, for one page. Next replaces a parent's
 * `openGraph` and `twitter` whole rather than merging them, so every page
 * gives all of it. Relative addresses are made absolute on `siteOrigin()`,
 * which the root layout sets as `metadataBase`.
 */
export function sharing({ title, description, path, image, imageAlt, type }: Shared): Metadata {
  const images = [{ url: image, ...cardSize, alt: imageAlt, type: "image/png" }];
  return {
    alternates: { canonical: path },
    openGraph: {
      type,
      siteName: site.name,
      locale: "en_GB",
      url: path,
      title,
      description,
      images,
    },
    twitter: {
      card: "summary_large_image",
      site: site.x,
      creator: site.x,
      title,
      description,
      images,
    },
  };
}
