import * as stylex from "@stylexjs/stylex";
import { Analytics } from "@vercel/analytics/next";
import type { Metadata, Viewport } from "next";
import type { ReactNode } from "react";

import { sharing, siteCardPath } from "@/lib/sharing";
import { site, siteOrigin } from "@/lib/site";
import { themeScript } from "@/lib/theme";
import { colors, typography } from "@/styles/shelf/tokens.stylex";

import "./globals.css";

const { openGraph, twitter } = sharing({
  title: site.title,
  description: site.description,
  path: "/",
  image: siteCardPath,
  imageAlt: `${site.name}: ${site.headline}`,
  type: "website",
  video: true,
});

/** What a page has when it says nothing of its own. Its address is left out, so a page is never shared as another. */
export const metadata: Metadata = {
  metadataBase: new URL(siteOrigin()),
  title: { default: site.title, template: `%s | ${site.name}` },
  description: site.description,
  applicationName: site.name,
  openGraph: { ...openGraph, url: undefined },
  twitter,
};

/** The browser's own bars, in the colour of the page under them. */
export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#fbfaf8" },
    { media: "(prefers-color-scheme: dark)", color: "#111110" },
  ],
};

export default function Layout({ children }: { children: ReactNode }) {
  return (
    // The script in <head> sets the theme on <html> before React hydrates.
    // `data-scroll-behavior` tells Next that `globals.css` scrolls smoothly, so
    // that it goes to the top of a new page at once. Without it the scroll to
    // the top is still under way when Next looks, and the page ends up under
    // the header.
    <html lang="en" data-scroll-behavior="smooth" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeScript() }} />
      </head>
      <body {...stylex.props(styles.body)}>
        {children}
        <Analytics />
      </body>
    </html>
  );
}

const styles = stylex.create({
  body: {
    fontSynthesis: "none",
    margin: 0,
    backgroundColor: colors.background,
    color: colors.foreground,
    fontFamily: typography.fontFamily,
    fontSize: typography.fontSizeBase,
    lineHeight: typography.lineHeightBase,
    textRendering: "optimizeLegibility",
    WebkitFontSmoothing: "antialiased",
  },
});
