"use client";

import * as stylex from "@stylexjs/stylex";
import Link from "next/link";
import { usePathname } from "next/navigation";

import * as Breadcrumb from "spool-ui/components/breadcrumb";
import type { NavGroup } from "@/lib/nav";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { media } from "spool-ui/conditions.stylex";
import { colors } from "spool-ui/tokens.stylex";

/** The group a page is in, then the page. */
export function DocsBreadcrumb({ groups }: { groups: NavGroup[] }) {
  const pathname = usePathname();
  const group = groups.find((candidate) => candidate.pages.some((page) => page.url === pathname));
  const page = group?.pages.find((candidate) => candidate.url === pathname);

  return (
    <Breadcrumb.Root>
      <Breadcrumb.List style={styles.list}>
        <Breadcrumb.Item style={styles.leading}>
          {/* Shelf's Breadcrumb.Link is a plain anchor; this one navigates without a reload. */}
          <Link href="/docs" {...stylex.props(styles.link)}>
            Docs
          </Link>
        </Breadcrumb.Item>
        {group && page ? (
          <>
            <Breadcrumb.Separator style={styles.leading} />
            <Breadcrumb.Item style={styles.leading}>{group.title}</Breadcrumb.Item>
            <Breadcrumb.Separator style={styles.leading} />
            <Breadcrumb.Item>
              <Breadcrumb.Page>{page.title}</Breadcrumb.Page>
            </Breadcrumb.Item>
          </>
        ) : null}
      </Breadcrumb.List>
    </Breadcrumb.Root>
  );
}

const styles = stylex.create({
  list: {
    flexWrap: "nowrap",
    whiteSpace: "nowrap",
    maxHeight: layout.headerHeight,
  },
  // On a phone only the page's own name fits beside the buttons.
  leading: {
    display: {
      default: "inline-flex",
      [breakpoints.mobile]: "none",
    },
  },
  link: {
    textDecoration: "none",
    color: {
      default: "inherit",
      ":hover": {
        default: null,
        [media.hover]: colors.foreground,
      },
    },
  },
});
