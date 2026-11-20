import * as stylex from "@stylexjs/stylex";
import type { MDXComponents } from "mdx/types";
import Image, { type ImageProps } from "next/image";
import Link from "next/link";
import type { ComponentProps } from "react";

import { CodeBlock } from "@/components/code-block";
import * as Table from "spool-ui/components/table";
import * as Typography from "spool-ui/components/typography";
import { layout } from "@/styles/layout.stylex";
import { colors, radius, spacing, typography } from "spool-ui/tokens.stylex";

const styles = stylex.create({
  heading: {
    scrollMarginTop: `calc(${layout.headerHeight} + ${spacing["6"]})`,
  },
  h2: {
    fontSize: "1.5rem",
    lineHeight: "2rem",
    marginTop: "3rem",
  },
  h3: {
    fontSize: "1.25rem",
    lineHeight: "1.75rem",
    marginTop: "2.25rem",
  },
  h4: {
    fontSize: typography.fontSizeBase,
    lineHeight: "1.5rem",
  },
  headingLink: {
    textDecoration: "none",
    color: "inherit",
  },
  block: {
    marginTop: spacing["4"],
  },
  list: {
    marginBlock: spacing["4"],
  },
  ordered: {
    marginBlock: spacing["4"],
    color: colors.foreground,
    fontSize: typography.fontSizeBase,
    lineHeight: "1.75rem",
    paddingInlineStart: spacing["6"],
  },
  item: {
    marginTop: spacing["1"],
  },
  link: {
    color: colors.foreground,
    fontWeight: typography.fontWeightMedium,
    textDecorationColor: colors.ring,
    textDecorationLine: "underline",
    textDecorationThickness: "1px",
    textUnderlineOffset: 4,
  },
  rule: {
    marginBlock: "2.5rem",
    borderWidth: 0,
    backgroundColor: colors.border,
    height: 1,
  },
  strong: {
    fontWeight: typography.fontWeightSemibold,
  },
  image: {
    marginBlock: spacing["6"],
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    display: "block",
    height: "auto",
    maxWidth: "100%",
  },
  table: {
    marginBlock: spacing["6"],
  },
  cell: {
    lineHeight: "1.5rem",
    verticalAlign: "top",
    whiteSpace: "normal",
  },
});

type HeadingProps = ComponentProps<"h2">;

/** A heading that links to itself, and stops clear of the header when jumped to. */
function heading(Tag: typeof Typography.H2, sizeStyle?: stylex.StaticStyles) {
  return function Heading({ id, children, className: _className, style: _style, ...props }: HeadingProps) {
    return (
      <Tag id={id} {...props} style={[styles.heading, sizeStyle]}>
        {id ? (
          <a href={`#${id}`} {...stylex.props(styles.headingLink)}>
            {children}
          </a>
        ) : (
          children
        )}
      </Tag>
    );
  };
}

function Anchor({ href = "", children }: ComponentProps<"a">) {
  const props = stylex.props(styles.link);
  if (href.startsWith("/") || href.startsWith("#")) {
    return (
      <Link href={href} {...props}>
        {children}
      </Link>
    );
  }
  return (
    <a href={href} {...props}>
      {children}
    </a>
  );
}

/** How each Markdown element is drawn. Pages use nothing else. */
export const mdxComponents: MDXComponents = {
  // Sized for a reading column: Shelf's defaults are a step larger.
  h2: heading(Typography.H2, styles.h2),
  h3: heading(Typography.H3, styles.h3),
  h4: heading(Typography.H4, styles.h4),
  p: ({ children }) => <Typography.P style={styles.block}>{children}</Typography.P>,
  ul: ({ children }) => <Typography.List style={styles.list}>{children}</Typography.List>,
  ol: ({ children, start }) => (
    <ol start={start} {...stylex.props(styles.ordered)}>
      {children}
    </ol>
  ),
  li: ({ children }) => <li {...stylex.props(styles.item)}>{children}</li>,
  blockquote: ({ children }) => <Typography.Blockquote>{children}</Typography.Blockquote>,
  a: Anchor,
  code: ({ children }) => <Typography.InlineCode>{children}</Typography.InlineCode>,
  pre: CodeBlock,
  // A page's image is a file under `public/`. The build reads its size, so the
  // page does not move when it loads.
  img: (props) => (
    <Image
      {...(props as ImageProps)}
      sizes="(width < 48rem) 100vw, 46rem"
      {...stylex.props(styles.image)}
    />
  ),
  hr: () => <hr {...stylex.props(styles.rule)} />,
  strong: ({ children }) => <strong {...stylex.props(styles.strong)}>{children}</strong>,
  table: ({ children }) => (
    <div {...stylex.props(styles.table)}>
      <Table.Root>{children}</Table.Root>
    </div>
  ),
  thead: ({ children }) => <Table.Header>{children}</Table.Header>,
  tbody: ({ children }) => <Table.Body>{children}</Table.Body>,
  tr: ({ children }) => <Table.Row>{children}</Table.Row>,
  th: ({ children }) => <Table.Head>{children}</Table.Head>,
  // Shelf's cells do not wrap, which suits data and not sentences.
  td: ({ children }) => <Table.Cell style={styles.cell}>{children}</Table.Cell>,
};
