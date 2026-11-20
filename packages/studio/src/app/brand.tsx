// The site's wordmark, as the door and the sidebar show it.

import * as stylex from "@stylexjs/stylex";

import { Mark, Wordmark } from "spool-ui/brand";
import { colors, display, linen, spacing, typography } from "spool-ui/tokens.stylex";

/** The site's wordmark, and what this is of Spool. Where the sidebar is narrow, the mark alone. */
export function Brand({ narrow = false }: { narrow?: boolean }) {
  if (narrow) return <Mark {...stylex.props(styles.mark)} aria-label="Spool studio" role="img" />;
  return (
    <span {...stylex.props(styles.brandName)}>
      <Wordmark {...stylex.props(styles.wordmark)} />
      <span {...stylex.props(styles.brandKind)}>studio</span>
    </span>
  );
}

const styles = stylex.create({
  mark: {
    color: colors.foreground,
    flexShrink: 0,
    height: "1.5rem",
    width: "1.5rem",
  },
  wordmark: {
    color: colors.foreground,
    display: "block",
    flexShrink: 0,
  },
  brandName: {
    gap: spacing["2"],
    alignItems: "center",
    display: "inline-flex",
  },
  // Set as the site sets its headlines, beside the wordmark.
  brandKind: {
    color: colors.mutedForeground,
    fontFamily: display.fontFamily,
    fontSize: typography.fontSizeLg,
    fontVariationSettings: '"opsz" 24',
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: 1,
    paddingTop: "0.125rem",
  },
});
