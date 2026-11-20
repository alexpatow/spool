// The styles the parts of a review card share: the card itself, and the
// table that sets what waits beside what it matched.

import * as stylex from "@stylexjs/stylex";

import { colors, display, linen, radius, spacing, typography } from "spool-ui/tokens.stylex";

export const shared = stylex.create({
  card: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    backgroundColor: colors.background,
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
    overflow: "hidden",
  },
  why: {
    margin: 0,
    color: colors.mutedForeground,
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    maxWidth: "46rem",
  },
  compare: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
  },
  row: {
    borderBottomWidth: { default: 1, ":last-child": 0 },
  },
  cell: {
    borderInlineStartColor: colors.border,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: { default: 1, ":first-child": 0 },
    paddingBlock: spacing["2"],
    paddingInline: spacing["4"],
    height: "auto",
    verticalAlign: "top",
    whiteSpace: "normal",
  },
  corner: {
    width: "1%",
  },
  column: {
    color: colors.foreground,
    fontWeight: typography.fontWeightRegular,
    minWidth: "13rem",
    paddingBlock: spacing["3"],
  },
  columnKind: {
    color: colors.mutedForeground,
    display: "block",
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
  },
  columnName: {
    display: "block",
    fontSize: typography.fontSizeBase,
    fontWeight: typography.fontWeightSemibold,
    lineHeight: typography.lineHeightBase,
    overflowWrap: "anywhere",
  },
  columnMeta: {
    gap: spacing["2"],
    alignItems: "center",
    color: colors.mutedForeground,
    display: "flex",
    flexWrap: "wrap",
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
    marginTop: spacing["1.5"],
  },
  waits: {
    backgroundColor: colors.card,
  },
  fieldName: {
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
    fontWeight: typography.fontWeightRegular,
    whiteSpace: "nowrap",
    width: "1%",
  },
  rowName: {
    color: colors.mutedForeground,
    fontSize: typography.fontSizeXs,
    fontWeight: typography.fontWeightMedium,
    verticalAlign: "top",
    whiteSpace: "nowrap",
    width: "1%",
  },
  value: {
    overflowWrap: "anywhere",
  },
  none: {
    color: colors.mutedForeground,
    fontStyle: "italic",
  },
  times: {
    color: colors.mutedForeground,
    fontVariantNumeric: "tabular-nums",
  },
  partTitle: {
    margin: 0,
    paddingBlock: spacing["3"],
    paddingInline: spacing["4"],
    color: colors.foreground,
    fontFamily: display.fontFamily,
    fontSize: typography.fontSizeLg,
    fontVariationSettings: '"opsz" 24',
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: typography.lineHeightLg,
  },
  answers: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    gap: spacing["2"],
    paddingBottom: spacing["4"],
    backgroundColor: colors.card,
    display: "flex",
    flexDirection: "column",
  },
  answerList: {
    margin: 0,
    paddingInline: spacing["4"],
    gap: spacing["2"],
    display: "flex",
    flexDirection: "column",
    listStyleType: "none",
  },
  checked: {
    paddingInline: spacing["4"],
  },
});
