import * as stylex from "@stylexjs/stylex";

import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "spool-ui/components/button";
import { site } from "@/lib/site";
import { spacing } from "spool-ui/tokens.stylex";

/** The right-hand end of both headers: the repository and the theme. */
export function SiteLinks() {
  return (
    <div {...stylex.props(styles.row)}>
      <Button variant="ghost" size="sm" nativeButton={false} render={<a href={site.repository} />}>
        GitHub
      </Button>
      <ThemeToggle />
    </div>
  );
}

const styles = stylex.create({
  row: {
    gap: spacing["1"],
    alignItems: "center",
    display: "flex",
    flexShrink: 0,
  },
});
