import * as stylex from "@stylexjs/stylex";
import type { Metadata } from "next";
import type { ReactNode } from "react";

import { site, siteOrigin } from "@/lib/site";
import { themeScript } from "@/lib/theme";
import { colors, typography } from "@/styles/shelf/tokens.stylex";

import "./globals.css";

export const metadata: Metadata = {
  metadataBase: new URL(siteOrigin()),
  title: { default: site.title, template: `%s | ${site.name}` },
  description: site.description,
};

export default function Layout({ children }: { children: ReactNode }) {
  return (
    // The script in <head> sets the theme on <html> before React hydrates.
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeScript() }} />
      </head>
      <body {...stylex.props(styles.body)}>{children}</body>
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
