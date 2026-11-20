// The dark page: the site's colours for it, and the studio's own tones.

import * as stylex from "@stylexjs/stylex";

import { darkLinen, darkTheme } from "spool-ui/themes";
import { tones } from "./tones.stylex";

const darkTones = stylex.createTheme(tones, {
  good: "oklch(0.8 0.14 155)",
  goodSurface: "oklch(0.27 0.05 155)",
  goodMark: "oklch(0.72 0.17 150)",
  waits: "oklch(0.84 0.12 85)",
  waitsSurface: "oklch(0.28 0.05 80)",
  waitsMark: "oklch(0.78 0.15 75)",
  bad: "oklch(0.76 0.15 22)",
  badSurface: "oklch(0.27 0.06 25)",
  badMark: "oklch(0.68 0.19 25)",
  plainMark: "#737373",
  source1: "oklch(0.7 0.15 255)",
  source2: "oklch(0.74 0.14 55)",
  source3: "oklch(0.72 0.14 160)",
  source4: "oklch(0.72 0.16 310)",
  source5: "oklch(0.76 0.12 200)",
  source6: "oklch(0.72 0.16 15)",
});

export type Theme = "light" | "dark";

const dark = [darkTheme, darkLinen, darkTones]
  .flatMap((theme) => (stylex.props(theme).className ?? "").split(" "))
  .filter(Boolean);

/**
 * Makes the whole page light or dark, with what is shown over it. Nothing
 * is written into the page but class names: its policy allows no style
 * that is not in its style sheet.
 */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): void {
  for (const name of dark) root.classList.toggle(name, theme === "dark");
  root.dataset["theme"] = theme;
  root.style.colorScheme = theme;
}
