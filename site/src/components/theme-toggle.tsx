"use client";

import { useSyncExternalStore } from "react";

import { Button } from "spool-ui/components/button";
import { DarkModeIcon, LightModeIcon } from "spool-ui/components/icons";
import { THEME_STORAGE_KEY } from "@/lib/theme";
import { type Theme, applyTheme } from "spool-ui/themes";

function subscribe(onChange: () => void): () => void {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributeFilter: ["data-theme"] });
  return () => observer.disconnect();
}

function current(): Theme {
  return document.documentElement.dataset["theme"] === "dark" ? "dark" : "light";
}

/** Switches between light and dark and remembers the choice. */
export function ThemeToggle() {
  const theme = useSyncExternalStore(subscribe, current, () => "light" as Theme);
  const next: Theme = theme === "dark" ? "light" : "dark";

  return (
    <Button
      variant="ghost"
      size="icon-sm"
      aria-label={`Switch to ${next} theme`}
      onClick={() => {
        applyTheme(next);
        try {
          localStorage.setItem(THEME_STORAGE_KEY, next);
        } catch {
          // Storage can be refused; the choice then lasts for this page only.
        }
      }}
    >
      {theme === "dark" ? <LightModeIcon /> : <DarkModeIcon />}
    </Button>
  );
}
