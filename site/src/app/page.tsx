import * as stylex from "@stylexjs/stylex";
import type { Metadata } from "next";
import Link from "next/link";

import { CodeBlock } from "@/components/code-block";
import { FirstScreen } from "@/components/first-screen";
import { SiteHeader } from "@/components/site-header";
import { Button } from "@/components/ui/button";
import * as Typography from "@/components/ui/typography";
import { explainer } from "@/lib/explainer";
import { sharing, siteCardPath } from "@/lib/sharing";
import { site } from "@/lib/site";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { colors, linen, spacing, typography } from "@/styles/shelf/tokens.stylex";

const server = `curl -fsSL ${site.repository}/releases/latest/download/install.sh | sh
spool init companies --example companies
cd companies
spool dev`;

const commands = `spool load samples/crm-1.csv
spool match
spool load samples/crm-2.csv
spool match
spool extract samples/notes-1.txt
spool match
spool show identities`;

const does = [
  "Keeps every row exactly as it arrived.",
  "Extracts records from text with your own code.",
  "Matches with SQL rules versioned in Git.",
  "Mints IDs that survive new data and better rules.",
  "Logs every merge, split and undo, with who and why.",
  "Explains each match and queues the hard cases.",
];

const waysIn = [
  "A CLI to load, match and explain.",
  "An HTTP API for your pipelines.",
  "An MCP endpoint for your agents.",
  "A studio for review and corrections.",
  "One Rust binary, one DuckDB file.",
];

export const metadata: Metadata = sharing({
  title: site.title,
  description: site.description,
  path: "/",
  image: siteCardPath,
  imageAlt: `${site.name}: ${site.headline}`,
  type: "website",
  video: true,
});

export default function Home() {
  return (
    <div {...stylex.props(styles.page)}>
      <SiteHeader />

      <main {...stylex.props(styles.main)}>
        <section {...stylex.props(styles.hero)}>
          <Link href={explainer.path} {...stylex.props(styles.newcomer)}>
            New to this? <span {...stylex.props(styles.newcomerTitle)}>{explainer.title}</span>
          </Link>
          <Typography.H1 style={styles.title}>{site.headline}</Typography.H1>
          <Typography.P style={styles.lead}>{site.summary}</Typography.P>
          <div {...stylex.props(styles.actions)}>
            <Button size="lg" nativeButton={false} render={<Link href="/docs/getting-started" />}>
              Get started
            </Button>
            <Button
              variant="outline"
              size="lg"
              nativeButton={false}
              render={<Link href="/docs" />}
            >
              Read the docs
            </Button>
          </div>
        </section>

        <section aria-label="The sample project" {...stylex.props(styles.screen)}>
          <FirstScreen />
          <Typography.Muted style={styles.screenCaption}>
            The sample project from{" "}
            <Link href="/docs/getting-started" {...stylex.props(styles.link)}>
              Getting started
            </Link>
            : nine CRM rows and one call note. &ldquo;Acorn Tools Ltd&rdquo; in the note joined the
            identity four CRM rows already had, and the ID stayed the same.
          </Typography.Muted>
        </section>

        <section aria-labelledby="start" {...stylex.props(styles.section, styles.columns)}>
          <div>
            <Typography.H2 id="start" style={styles.heading}>
              Run it
            </Typography.H2>
            <Typography.P>
              Install it, make the sample project and start the server on your machine.
            </Typography.P>
            <CodeBlock>{server}</CodeBlock>
            <Typography.Muted>
              Linux or macOS on Apple silicon. Version {site.version}, {site.licence} licence.
            </Typography.Muted>
          </div>
          <div>
            <Typography.H2 style={styles.heading}>Then, in a second terminal</Typography.H2>
            <Typography.P>
              Load the rows, match, and extract the companies from the note. These seven commands
              produce the picture above.
            </Typography.P>
            <CodeBlock>{commands}</CodeBlock>
            <Typography.Muted>
              <Link href="/docs/getting-started" {...stylex.props(styles.link)}>
                Getting started
              </Link>{" "}
              shows what each prints, and the same steps over HTTP and MCP.
            </Typography.Muted>
          </div>
        </section>

        <section aria-label="Scope" {...stylex.props(styles.section, styles.columns)}>
          <div>
            <Typography.H2 style={styles.heading}>What it does</Typography.H2>
            <ul {...stylex.props(styles.list)}>
              {does.map((item) => (
                <li key={item} {...stylex.props(styles.item)}>
                  {item}
                </li>
              ))}
            </ul>
          </div>
          <div>
            <Typography.H2 style={styles.heading}>How you use it</Typography.H2>
            <ul {...stylex.props(styles.list)}>
              {waysIn.map((item) => (
                <li key={item} {...stylex.props(styles.item)}>
                  {item}
                </li>
              ))}
            </ul>
            <Typography.Muted style={styles.caption}>
              Benchmarked to 5,000,000 records on one machine.{" "}
              <Link href="/docs/limits#benchmarks" {...stylex.props(styles.link)}>
                See the numbers
              </Link>
              .
            </Typography.Muted>
          </div>
        </section>
      </main>
    </div>
  );
}

const styles = stylex.create({
  page: {
    display: "flex",
    flexDirection: "column",
    minHeight: "100svh",
  },
  newcomer: {
    gap: spacing["2"],
    marginBottom: "1.75rem",
    paddingBlock: spacing["1.5"],
    paddingInline: spacing["3"],
    alignItems: "baseline",
    borderColor: {
      default: colors.border,
      ":hover": linen.thread,
    },
    borderStyle: "solid",
    borderWidth: 1,
    color: colors.mutedForeground,
    display: "inline-flex",
    flexWrap: "wrap",
    fontSize: "0.8125rem",
    justifyContent: "center",
    lineHeight: "1.25rem",
    textDecoration: "none",
    transitionDuration: "150ms",
    transitionProperty: "border-color",
  },
  newcomerTitle: {
    color: linen.thread,
  },
  main: {
    marginInline: "auto",
    paddingBlock: {
      default: "5rem",
      [breakpoints.mobile]: "3rem",
    },
    paddingInline: {
      default: "2.5rem",
      [breakpoints.mobile]: spacing["4"],
    },
    boxSizing: "border-box",
    maxWidth: layout.pageWidth,
    width: "100%",
  },
  hero: {
    marginInline: "auto",
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    maxWidth: "40rem",
    textAlign: "center",
  },
  title: {
    fontSize: {
      default: "3.25rem",
      [breakpoints.mobile]: "2.25rem",
    },
    lineHeight: 1.08,
  },
  lead: {
    marginTop: spacing["6"],
    color: colors.mutedForeground,
    fontSize: "1.125rem",
    lineHeight: "1.75rem",
    textWrap: "pretty",
  },
  actions: {
    gap: spacing["3"],
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "center",
    marginTop: "2rem",
  },
  screen: {
    marginTop: "4rem",
  },
  screenCaption: {
    marginInline: "auto",
    marginTop: spacing["4"],
    maxWidth: "44rem",
    textAlign: "center",
  },
  section: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    marginTop: "4.5rem",
    paddingTop: "3rem",
  },
  heading: {
    fontSize: "1.375rem",
  },
  caption: {
    marginTop: spacing["4"],
  },
  columns: {
    gap: "3rem",
    display: "grid",
    gridTemplateColumns: {
      default: "minmax(0, 1fr) minmax(0, 1fr)",
      [breakpoints.narrow]: "minmax(0, 1fr)",
    },
  },
  list: {
    margin: 0,
    marginTop: spacing["4"],
    paddingInlineStart: "1.25rem",
  },
  item: {
    marginTop: spacing["2"],
    color: colors.foreground,
    fontSize: "0.9375rem",
    lineHeight: "1.625rem",
    "::marker": {
      color: linen.faint,
    },
  },
  link: {
    color: colors.foreground,
    textDecorationColor: linen.lineStrong,
    textDecorationLine: "underline",
    textUnderlineOffset: 4,
  },
});
