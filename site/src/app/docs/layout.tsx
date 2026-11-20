import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { DocsBreadcrumb } from "@/components/docs-breadcrumb";
import { DocsSidebar } from "@/components/docs-sidebar";
import { SiteLinks } from "@/components/site-links";
import { Separator } from "spool-ui/components/separator";
import { SidebarInset, SidebarProvider, SidebarTrigger } from "spool-ui/components/sidebar";
import { navGroups } from "@/lib/nav";
import { layout } from "@/styles/layout.stylex";
import { colors, spacing } from "spool-ui/tokens.stylex";

export default function DocsLayout({ children }: { children: ReactNode }) {
  const groups = navGroups();

  return (
    <SidebarProvider>
      <DocsSidebar groups={groups} />
      <SidebarInset>
        <header {...stylex.props(styles.header)}>
          <SidebarTrigger />
          <Separator orientation="vertical" style={styles.divider} />
          <DocsBreadcrumb groups={groups} />
          <div {...stylex.props(styles.spacer)} />
          <SiteLinks />
        </header>
        {children}
      </SidebarInset>
    </SidebarProvider>
  );
}

const styles = stylex.create({
  header: {
    gap: spacing["2"],
    paddingInline: spacing["4"],
    alignItems: "center",
    backgroundColor: `color-mix(in oklab, ${colors.background} 85%, transparent)`,
    backdropFilter: "blur(8px)",
    borderBottomColor: colors.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    boxSizing: "border-box",
    display: "flex",
    flexShrink: 0,
    position: "sticky",
    zIndex: 10,
    height: layout.headerHeight,
    top: 0,
  },
  divider: {
    alignSelf: "center",
    marginInlineEnd: spacing["2"],
    height: "1rem",
  },
  spacer: {
    flexGrow: 1,
    minWidth: 0,
  },
});
