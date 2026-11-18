"use client";

import * as stylex from "@stylexjs/stylex";
import Link from "next/link";
import { usePathname } from "next/navigation";

import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar";
import type { NavGroup } from "@/lib/nav";
import { site } from "@/lib/site";
import { layout } from "@/styles/layout.stylex";
import { colors, spacing, typography } from "@/styles/shelf/tokens.stylex";

/** The page tree, on the left. Below 48rem Shelf turns it into a drawer. */
export function DocsSidebar({ groups }: { groups: NavGroup[] }) {
  const pathname = usePathname();
  const { setOpenMobile } = useSidebar();
  const close = () => setOpenMobile(false);

  return (
    <Sidebar>
      <SidebarHeader style={styles.header}>
        <Link href="/" onClick={close} {...stylex.props(styles.wordmark)}>
          {site.name}
        </Link>
      </SidebarHeader>
      <SidebarContent>
        <nav aria-label="Documentation">
          {groups.map((group) => (
            <SidebarGroup key={group.title}>
              <SidebarGroupLabel>{group.title}</SidebarGroupLabel>
              <SidebarMenu>
                {group.pages.map((page) => (
                  <SidebarMenuItem key={page.url}>
                    <SidebarMenuButton
                      isActive={pathname === page.url}
                      render={<Link href={page.url} />}
                      onClick={close}
                    >
                      {page.title}
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroup>
          ))}
        </nav>
      </SidebarContent>
    </Sidebar>
  );
}

const styles = stylex.create({
  header: {
    paddingInline: spacing["4"],
    borderBottomColor: colors.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    boxSizing: "border-box",
    flexShrink: 0,
    justifyContent: "center",
    height: layout.headerHeight,
  },
  wordmark: {
    textDecoration: "none",
    color: colors.foreground,
    fontSize: typography.fontSizeLg,
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.02em",
  },
});
