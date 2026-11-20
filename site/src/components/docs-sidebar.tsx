"use client";

import * as stylex from "@stylexjs/stylex";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useRef } from "react";

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
} from "spool-ui/components/sidebar";
import { Wordmark } from "spool-ui/brand";
import { explainer } from "@/lib/explainer";
import type { NavGroup } from "@/lib/nav";
import { layout } from "@/styles/layout.stylex";
import { colors, linen, spacing, typography } from "spool-ui/tokens.stylex";

/** The page tree, on the left. Below 48rem Shelf turns it into a drawer. */
export function DocsSidebar({ groups }: { groups: NavGroup[] }) {
  const pathname = usePathname();
  const { setOpenMobile } = useSidebar();
  const close = () => setOpenMobile(false);
  const nav = useRef<HTMLElement>(null);

  // The sidebar scrolls on its own. A page opened by its address may be one
  // that is out of sight there, so its item is brought into view.
  useEffect(() => {
    nav.current?.querySelector('[aria-current="page"]')?.scrollIntoView({ block: "nearest" });
  }, [pathname]);

  return (
    <Sidebar>
      <SidebarHeader style={styles.header}>
        <Link href="/" onClick={close} {...stylex.props(styles.wordmark)}>
          <Wordmark />
        </Link>
      </SidebarHeader>
      <SidebarContent>
        <nav aria-label="Documentation" ref={nav}>
          <Link href={explainer.path} onClick={close} {...stylex.props(styles.newcomer)}>
            <span {...stylex.props(styles.newcomerLabel)}>New here?</span>
            {explainer.title}
          </Link>
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
    display: "flex",
  },
  newcomer: {
    gap: 2,
    marginBlock: spacing["3"],
    marginInline: spacing["2"],
    paddingBlock: spacing["2"],
    paddingInline: spacing["3"],
    borderColor: {
      default: colors.border,
      ":hover": linen.thread,
    },
    borderStyle: "solid",
    borderWidth: 1,
    color: colors.foreground,
    display: "flex",
    flexDirection: "column",
    fontSize: "0.8125rem",
    lineHeight: "1.125rem",
    textDecoration: "none",
    transitionDuration: "150ms",
    transitionProperty: "border-color",
  },
  newcomerLabel: {
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.6875rem",
    letterSpacing: "0.06em",
    textTransform: "uppercase",
  },
});
