"use client";

import * as stylex from "@stylexjs/stylex";
import { AnchorProvider, TOCItem, type TOCItemType, useActiveAnchor } from "fumadocs-core/toc";

import { layout, breakpoints } from "@/styles/layout.stylex";
import { media } from "spool-ui/conditions.stylex";
import { colors, spacing, typography } from "spool-ui/tokens.stylex";

/** The headings of the page, on the right, with the one in view marked. */
export function TableOfContents({ items }: { items: TOCItemType[] }) {
  if (items.length === 0) return <aside {...stylex.props(styles.aside)} />;

  return (
    <aside {...stylex.props(styles.aside)}>
      <nav aria-label="On this page" {...stylex.props(styles.nav)}>
        <div {...stylex.props(styles.label)}>On this page</div>
        <AnchorProvider toc={items} single>
          <Items items={items} />
        </AnchorProvider>
      </nav>
    </aside>
  );
}

function Items({ items }: { items: TOCItemType[] }) {
  const active = useActiveAnchor();

  return (
    <ul {...stylex.props(styles.list)}>
      {items.map((item) => (
        <li key={item.url}>
          <TOCItem
            href={item.url}
            {...stylex.props(styles.item, item.depth > 2 && styles.nested, item.url === `#${active}` && styles.active)}
          >
            {item.title}
          </TOCItem>
        </li>
      ))}
    </ul>
  );
}

const styles = stylex.create({
  aside: {
    display: {
      default: "block",
      [breakpoints.narrow]: "none",
    },
    flexShrink: 0,
    width: layout.tocWidth,
  },
  nav: {
    paddingBlock: "2.5rem",
    boxSizing: "border-box",
    overflowY: "auto",
    position: "sticky",
    maxHeight: `calc(100svh - ${layout.headerHeight})`,
    top: layout.headerHeight,
  },
  label: {
    color: colors.foreground,
    fontSize: typography.fontSizeSm,
    fontWeight: typography.fontWeightMedium,
    lineHeight: typography.lineHeightSm,
    marginBottom: spacing["2"],
  },
  list: {
    margin: 0,
    padding: 0,
    listStyle: "none",
    borderInlineStartColor: colors.border,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: 1,
  },
  item: {
    paddingBlock: spacing["1"],
    textDecoration: "none",
    borderInlineStartColor: "transparent",
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: 1,
    color: {
      default: colors.mutedForeground,
      ":hover": {
        default: null,
        [media.hover]: colors.foreground,
      },
    },
    display: "block",
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    marginInlineStart: -1,
    paddingInlineStart: spacing["3"],
  },
  nested: {
    paddingInlineStart: spacing["6"],
  },
  active: {
    borderInlineStartColor: colors.foreground,
    color: colors.foreground,
  },
});
