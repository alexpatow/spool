import * as stylex from "@stylexjs/stylex";

import { darkLinen, darkTheme } from "spool-ui/themes";

export const THEME_STORAGE_KEY = "spool-theme";

/**
 * Runs in `<head>` before the first paint, so a reader who chose dark (or
 * whose system is dark) never sees a light page. It does what Shelf's
 * `applyTheme` does, without the transition guard a first paint does not need.
 */
export function themeScript(): string {
  const classes = JSON.stringify((stylex.props(darkTheme, darkLinen).className ?? "").split(" ").filter(Boolean));
  return `(function(){var t;try{t=localStorage.getItem(${JSON.stringify(THEME_STORAGE_KEY)})}catch(e){}if(t!=="light"&&t!=="dark"){t=matchMedia("(prefers-color-scheme: dark)").matches?"dark":"light"}var r=document.documentElement;if(t==="dark"){r.classList.add.apply(r.classList,${classes})}r.dataset.theme=t;r.style.colorScheme=t})()`;
}
