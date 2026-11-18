import * as stylex from "@stylexjs/stylex";

/**
 * The site's own measurements. Colour, type and spacing are Shelf's tokens in
 * `shelf/tokens.stylex.ts`; this file only says how wide and how tall the
 * page furniture is.
 */
export const layout = stylex.defineConsts({
  headerHeight: "3.5rem",
  /** The reading column. */
  proseWidth: "46rem",
  tocWidth: "14rem",
  pageWidth: "72rem",
});

export const breakpoints = stylex.defineConsts({
  /** Below this the table of contents is hidden. */
  narrow: "@media (width < 72rem)",
  /** Below this Shelf's sidebar becomes a drawer. */
  mobile: "@media (width < 48rem)",
});
