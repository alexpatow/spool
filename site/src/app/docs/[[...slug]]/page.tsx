import * as stylex from "@stylexjs/stylex";
import type { Metadata } from "next";
import Link from "next/link";
import { notFound } from "next/navigation";

import { mdxComponents } from "@/components/mdx";
import { PageActions } from "@/components/page-actions";
import { TableOfContents } from "@/components/table-of-contents";
import * as Typography from "@/components/ui/typography";
import { markdownPath } from "@/lib/markdown";
import { type NavPage, neighbours } from "@/lib/nav";
import { docsCardPath, sharing } from "@/lib/sharing";
import { site } from "@/lib/site";
import { source } from "@/lib/source";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { media } from "@/styles/shelf/conditions.stylex";
import { colors, radius, spacing, typography } from "@/styles/shelf/tokens.stylex";

type Props = { params: Promise<{ slug?: string[] }> };

export default async function Page({ params }: Props) {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();
  const Body = page.data.body;
  const { previous, next } = neighbours(page.url);

  return (
    <div {...stylex.props(styles.columns)}>
      <article {...stylex.props(styles.article)}>
        <header {...stylex.props(styles.head)}>
          <Typography.H1>{page.data.title}</Typography.H1>
          <Typography.Lead>{page.data.description}</Typography.Lead>
          <PageActions markdownUrl={markdownPath(page.url)} />
        </header>
        <Body components={mdxComponents} />
        <nav aria-label="Previous and next page" {...stylex.props(styles.pager)}>
          {previous ? <PagerLink page={previous} label="Previous" /> : <span />}
          {next ? <PagerLink page={next} label="Next" end /> : <span />}
        </nav>
      </article>
      <TableOfContents items={page.data.toc} />
    </div>
  );
}

function PagerLink({ page, label, end }: { page: NavPage; label: string; end?: boolean }) {
  return (
    <Link href={page.url} {...stylex.props(styles.pagerLink, end && styles.pagerEnd)}>
      <span {...stylex.props(styles.pagerLabel)}>{label}</span>
      <span {...stylex.props(styles.pagerTitle)}>{page.title}</span>
    </Link>
  );
}

export function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();
  const description = page.data.description ?? "";
  const shared = sharing({
    title: `${page.data.title} | ${site.name}`,
    description,
    path: page.url,
    image: docsCardPath(page.url),
    imageAlt: `${page.data.title}: ${description}`,
    type: "article",
  });
  return {
    ...shared,
    title: page.data.title,
    description,
    alternates: {
      ...shared.alternates,
      // An agent that fetched the HTML finds the Markdown here.
      types: { "text/markdown": markdownPath(page.url) },
    },
  };
}

const styles = stylex.create({
  columns: {
    gap: "3rem",
    marginInline: "auto",
    paddingInline: {
      default: "2.5rem",
      [breakpoints.mobile]: spacing["4"],
    },
    boxSizing: "border-box",
    display: "flex",
    justifyContent: "center",
    maxWidth: layout.pageWidth,
    width: "100%",
  },
  article: {
    paddingBlock: "2.5rem",
    flexGrow: 1,
    maxWidth: layout.proseWidth,
    minWidth: 0,
  },
  head: {
    borderBottomColor: colors.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    marginBottom: "2rem",
    paddingBottom: "2rem",
  },
  pager: {
    gap: spacing["4"],
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "grid",
    gridTemplateColumns: {
      default: "1fr 1fr",
      [breakpoints.mobile]: "1fr",
    },
    marginTop: "4rem",
    paddingTop: "2rem",
  },
  pagerLink: {
    padding: spacing["4"],
    borderColor: {
      default: colors.border,
      ":hover": {
        default: null,
        [media.hover]: colors.ring,
      },
    },
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["1"],
    textDecoration: "none",
    display: "flex",
    flexDirection: "column",
  },
  pagerEnd: {
    alignItems: "flex-end",
    textAlign: "end",
  },
  pagerLabel: {
    color: colors.mutedForeground,
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
  },
  pagerTitle: {
    color: colors.foreground,
    fontWeight: typography.fontWeightMedium,
  },
});
