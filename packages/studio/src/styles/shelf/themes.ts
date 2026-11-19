// Copied from site/src/styles/shelf/themes.ts by packages/studio/scripts/build.ts.
// Do not edit: change the site's file and run `bun run build` in packages/studio.

import * as stylex from "@stylexjs/stylex";
import { colors, linen } from "./tokens.stylex";

/**
 * Dark theme. Light is the default and needs no theme.
 *
 * Switch the whole app with `applyTheme("dark")`, so popups that render in a
 * portal, such as dialogs, get the theme too. To theme only a section, apply
 * it to that element: `<div {...stylex.props(darkTheme)}>…</div>`.
 */
export const darkTheme = stylex.createTheme(colors, {
  background: "#111110",
  foreground: "#ecebe7",
  card: "#161615",
  cardForeground: "#ecebe7",
  popover: "#161615",
  popoverForeground: "#ecebe7",
  primary: "#ecebe7",
  primaryForeground: "#111110",
  secondary: "#1e1e1c",
  secondaryForeground: "#ecebe7",
  muted: "#1e1e1c",
  mutedForeground: "#9a978f",
  accent: "#252523",
  accentForeground: "#ecebe7",
  destructive: "oklch(0.704 0.191 22.2)",
  destructiveForeground: "#ffffff",
  destructiveSurface: "oklch(0.53 0.22 27.3)",
  destructiveText: "oklch(0.704 0.191 22.2)",
  border: "#292826",
  input: "#3b3a37",
  ring: "#6a6862",
  overlay: "rgb(0 0 0 / 60%)",
  chart1: "oklch(0.98 0 0)",
  chart2: "oklch(0.78 0 0)",
  chart3: "oklch(0.62 0 0)",
  chart4: "oklch(0.5 0 0)",
  chart5: "oklch(0.4 0 0)",
  chart6: "oklch(0.32 0 0)",
  chartGrid: "#3b3a37",
  chartCursor: "rgb(255 255 255 / 7%)",
});

export const darkLinen = stylex.createTheme(linen, {
  thread: "#d08a75",
  threadWash: "#261b18",
  faint: "#6a6862",
  lineStrong: "#3b3a37",
  displayWeight: "360",
});

export type Theme = "light" | "dark";

const darkClassNames = (stylex.props(darkTheme, darkLinen).className ?? "").split(" ").filter(Boolean);

/**
 * Applies a theme to `root` (the whole document by default) without animating
 * controls to their new colors.
 */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): void {
  suppressTransitions();

  for (const className of darkClassNames) root.classList.toggle(className, theme === "dark");
  root.dataset["theme"] = theme;
  root.style.colorScheme = theme;
}

/** Turns transitions off for two frames, long enough for a theme change to render. */
function suppressTransitions(): void {
  const style = document.createElement("style");
  style.textContent = "*,*::before,*::after{transition:none!important}";
  document.head.append(style);

  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      // Resolve the new colors while transitions are still off.
      void getComputedStyle(document.body).opacity;
      style.remove();
    }),
  );
}
