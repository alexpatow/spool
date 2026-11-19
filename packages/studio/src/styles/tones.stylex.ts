import * as stylex from "@stylexjs/stylex";

/**
 * What a colour says in the studio. The components are grey on purpose, so
 * that these stand out: something holds, something waits for a person,
 * something was refused. `themes.ts` has the values for a dark page.
 */
export const tones = stylex.defineVars({
  good: "oklch(0.44 0.11 155)",
  goodSurface: "oklch(0.965 0.035 155)",
  goodMark: "oklch(0.64 0.17 150)",
  waits: "oklch(0.46 0.1 65)",
  waitsSurface: "oklch(0.965 0.05 90)",
  waitsMark: "oklch(0.76 0.16 75)",
  bad: "oklch(0.5 0.2 27.3)",
  badSurface: "oklch(0.965 0.025 27)",
  badMark: "oklch(0.63 0.22 27)",
  plainMark: "#a3a3a3",
  /** Where a source's records are told apart from another's. */
  source1: "oklch(0.62 0.17 255)",
  source2: "oklch(0.68 0.16 50)",
  source3: "oklch(0.62 0.15 160)",
  source4: "oklch(0.6 0.19 310)",
  source5: "oklch(0.7 0.14 200)",
  source6: "oklch(0.64 0.19 15)",
});
