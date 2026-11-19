import * as stylex from "@stylexjs/stylex";

/**
 * Semantic colors. Each surface token pairs with a `*Foreground` token for the
 * text and icons on it. Light values are the default; see `themes.ts` for dark.
 */
export const colors = stylex.defineVars({
  background: "#fbfaf8",
  foreground: "#1a1917",
  card: "#ffffff",
  cardForeground: "#1a1917",
  popover: "#ffffff",
  popoverForeground: "#1a1917",
  primary: "#1a1917",
  primaryForeground: "#fbfaf8",
  secondary: "#f2f0eb",
  secondaryForeground: "#1a1917",
  muted: "#f2f0eb",
  /** 4.5:1 against `background`, `card`, and `muted`. */
  mutedForeground: "#6b6862",
  accent: "#eceae4",
  accentForeground: "#1a1917",
  destructive: "oklch(0.577 0.245 27.3)",
  destructiveForeground: "#ffffff",
  /** Filled destructive controls; darker than `destructive` to keep 4.5:1 text contrast. */
  destructiveSurface: "oklch(0.53 0.22 27.3)",
  /** Error messages; 4.5:1 against both `background` and `card`. */
  destructiveText: "oklch(0.5 0.2 27.3)",
  border: "#e4e1da",
  input: "#cfcbc2",
  /** Focus rings: 3:1 against `background`, `card`, and `muted`. */
  ring: "#85817a",
  /** Covers the page behind modal surfaces such as dialogs. */
  overlay: "rgb(0 0 0 / 40%)",
  /**
   * Data series, in order, from strongest to faintest. Monochrome by default: tell series apart
   * with the `pattern` prop (solid, hatch, dots), or override these for a colored palette.
   */
  chart1: "oklch(0.2 0 0)",
  chart2: "oklch(0.42 0 0)",
  chart3: "oklch(0.58 0 0)",
  chart4: "oklch(0.7 0 0)",
  chart5: "oklch(0.8 0 0)",
  chart6: "oklch(0.88 0 0)",
  /** Gridlines behind a chart. Quieter than `border`, still 1.5:1 against `background`. */
  chartGrid: "#cfcbc2",
  /** The band or line that follows the pointer across a chart. */
  chartCursor: "rgb(0 0 0 / 5%)",
});

/**
 * Typefaces. `fonts.css` loads the defaults. To use your own, define `--font-sans` and
 * `--font-mono` (with next/font: `variable: "--font-sans"`), or edit these two values.
 */
export const typography = stylex.defineVars({
  fontFamily: 'var(--font-sans, "Hanken Grotesk Variable"), ui-sans-serif, system-ui, sans-serif',
  /** Code, keyboard keys, and one-time codes. */
  fontFamilyMono: 'var(--font-mono, "Commit Mono"), ui-monospace, SFMono-Regular, Menlo, monospace',
  fontSizeXs: "0.75rem",
  lineHeightXs: "1rem",
  fontSizeSm: "0.875rem",
  lineHeightSm: "1.25rem",
  fontSizeBase: "1rem",
  lineHeightBase: "1.5rem",
  fontSizeLg: "1.125rem",
  lineHeightLg: "1.75rem",
  // Components set `fontSynthesis: "none"`, so a typeface without a weight never renders a faux bold.
  fontWeightRegular: "400",
  fontWeightMedium: "500",
  fontWeightSemibold: "500",
});

/** A 0.25rem scale keyed by multiples, so `2.5` is 0.625rem. */
export const spacing = stylex.defineVars({
  "1": "0.25rem",
  "1.5": "0.375rem",
  "2": "0.5rem",
  "2.5": "0.625rem",
  "3": "0.75rem",
  "4": "1rem",
  "6": "1.5rem",
});

/** `sm` for small parts such as checkboxes and menu items, `md` for controls, `lg` for surfaces. */
export const radius = stylex.defineVars({
  none: "0",
  sm: "0",
  md: "0",
  lg: "0",
  full: "9999px",
});

export const elevation = stylex.defineVars({
  none: "none",
  xs: "none",
  sm: "none",
  lg: "none",
});

export const motion = stylex.defineVars({
  durationFast: "150ms",
  /** Panels that travel across the screen, such as drawers. */
  durationSlow: "300ms",
  easingStandard: "cubic-bezier(0.4, 0, 0.2, 1)",
  /** Fast out, soft landing: things that arrive, such as toasts. */
  easingOut: "cubic-bezier(0.22, 1, 0.36, 1)",
});

/** Control heights shared by interactive components. */
export const sizes = stylex.defineVars({
  controlXs: "1.5rem",
  controlSm: "1.75rem",
  controlDefault: "2rem",
  controlLg: "2.25rem",
});

/**
 * What the site adds to Shelf's colours. `thread` is the one colour, and it is
 * used only for an identity and for the log. Light values are the default; see
 * `themes.ts` for dark.
 */
export const linen = stylex.defineVars({
  thread: "#a4442f",
  threadWash: "#f6ebe7",
  /** Text quieter than `mutedForeground`: positions, counts, rulers. */
  faint: "#9d9990",
  lineStrong: "#cfcbc2",
  /** The weight of a headline, which is lighter on a dark ground. */
  displayWeight: "380",
});

/** Headlines only. Text is `typography.fontFamily`. */
export const display = stylex.defineConsts({
  fontFamily: '"Newsreader Variable", "Iowan Old Style", Georgia, serif',
  tracking: "-0.022em",
});
