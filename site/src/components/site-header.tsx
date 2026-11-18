import * as stylex from "@stylexjs/stylex";
import Link from "next/link";

import { Wordmark } from "@/components/brand";
import { SiteLinks } from "@/components/site-links";
import { Button } from "@/components/ui/button";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { colors, spacing } from "@/styles/shelf/tokens.stylex";

const links = [
  { href: "/docs", label: "Docs" },
  { href: "/docs/getting-started", label: "Getting started" },
  { href: "/docs/limits", label: "Limits" },
];

/** The header of the pages outside the docs: the front page and the explainer. */
export function SiteHeader() {
  return (
    <header {...stylex.props(styles.header)}>
      <div {...stylex.props(styles.inner)}>
        <Link href="/" {...stylex.props(styles.wordmark)}>
          <Wordmark />
        </Link>
        <nav aria-label="Site" {...stylex.props(styles.nav)}>
          {links.map((link) => (
            <Button
              key={link.href}
              variant="ghost"
              size="sm"
              nativeButton={false}
              render={<Link href={link.href} />}
            >
              {link.label}
            </Button>
          ))}
        </nav>
        <SiteLinks />
      </div>
    </header>
  );
}

const styles = stylex.create({
  header: {
    borderBottomColor: colors.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
  },
  inner: {
    gap: spacing["6"],
    marginInline: "auto",
    paddingInline: {
      default: "2.5rem",
      [breakpoints.mobile]: spacing["4"],
    },
    alignItems: "center",
    boxSizing: "border-box",
    display: "flex",
    height: layout.headerHeight,
    maxWidth: layout.pageWidth,
  },
  wordmark: {
    textDecoration: "none",
    color: colors.foreground,
    display: "flex",
  },
  nav: {
    display: {
      default: "flex",
      [breakpoints.mobile]: "none",
    },
    flexGrow: 1,
  },
});
