// The styles that more than one kind of piece is set in: a table is a
// panel, an ID is code.

import * as stylex from "@stylexjs/stylex";

import { colors, radius, spacing, typography } from "spool-ui/tokens.stylex";

export const shared = stylex.create({
  count: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamily,
    fontSize: typography.fontSizeSm,
    fontVariantNumeric: "tabular-nums",
    fontWeight: typography.fontWeightRegular,
    letterSpacing: "normal",
  },
  panel: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    backgroundColor: colors.background,
    overflow: "hidden",
  },
  code: {
    borderRadius: radius.sm,
    paddingBlock: "0.0625rem",
    paddingInline: spacing["1"],
    backgroundColor: colors.muted,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
    overflowWrap: "anywhere",
  },
  more: {
    display: "flex",
    justifyContent: "center",
  },
});
