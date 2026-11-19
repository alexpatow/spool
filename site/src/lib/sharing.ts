import type { Metadata } from "next";

import { absoluteUrl, site } from "@/lib/site";

/** The size of every card, which is what X, LinkedIn, Slack and Discord all crop to. */
export const cardSize = { width: 1200, height: 630 } as const;

/** Where the card of the front page is, and of every other page that has none of its own. */
export const siteCardPath = "/og/image.png";

/**
 * The video a link preview plays, where it plays one, in place of the card:
 * the problem, how Spool solves it, and how it connects to your tools.
 * `video/` in the site makes it.
 */
export const siteVideo = { path: "/videos/spool.mp4", width: 1920, height: 1080, type: "video/mp4" } as const;

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
  /** Whether a preview that plays video plays `siteVideo`. The card stays, for one that does not. */
  video?: boolean;
}

/**
 * What a link preview reads, for one page. Next replaces a parent's
 * `openGraph` and `twitter` whole rather than merging them, so every page
 * gives all of it. Relative addresses are made absolute on `siteOrigin()`,
 * which the root layout sets as `metadataBase`.
 */
export function sharing({ title, description, path, image, imageAlt, type, video = false }: Shared): Metadata {
  const images = [{ url: image, ...cardSize, alt: imageAlt, type: "image/png" }];
  // Next makes an image's address absolute on `metadataBase`, but not a video's, so these are made absolute here.
  const videos = video
    ? [{ url: absoluteUrl(siteVideo.path), secureUrl: absoluteUrl(siteVideo.path), type: siteVideo.type, width: siteVideo.width, height: siteVideo.height }]
    : undefined;
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
      ...(videos && { videos }),
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
