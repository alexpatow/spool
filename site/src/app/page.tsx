import * as stylex from "@stylexjs/stylex";
import type { Metadata } from "next";
import Link from "next/link";

import { CodeBlock } from "@/components/code-block";
import { FirstScreen } from "@/components/first-screen";
import { SiteHeader } from "@/components/site-header";
import { Button } from "spool-ui/components/button";
import * as Typography from "spool-ui/components/typography";
import { explainer } from "@/lib/explainer";
import { sharing, siteCardPath } from "@/lib/sharing";
import { site } from "@/lib/site";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { colors, linen, spacing } from "spool-ui/tokens.stylex";

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

const properties = [
  {
    title: "Persistent identities",
    description: "Store a Spool ID in your application and keep using it as more records join the identity.",
  },
  {
    title: "Explainable decisions",
    description: "See which records, rules and runs led to a match. Send ambiguous cases to review.",
  },
  {
    title: "Reversible by design",
    description: "Merge or split identities and undo corrections, with a history of who changed what and why.",
  },
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
          <Typography.H1 style={styles.title}>{site.headline}</Typography.H1>
          <Typography.P style={styles.lead}>{site.summary}</Typography.P>
          <div {...stylex.props(styles.actions)}>
            <Button size="lg" nativeButton={false} render={<Link href="/docs/getting-started" />}>
              Get started
            </Button>
            <Button variant="outline" size="lg" nativeButton={false} render={<Link href="/docs" />}>
              Read the docs
            </Button>
          </div>
        </section>

        <div {...stylex.props(styles.screen)}>
          <FirstScreen />
          <div {...stylex.props(styles.screenLinks)}>
            <Link href={explainer.path} {...stylex.props(styles.newcomer)}>
              New to this? <span {...stylex.props(styles.newcomerTitle)}>{explainer.title}</span>
            </Link>
          </div>
        </div>

        <section aria-labelledby="agents" {...stylex.props(styles.section)}>
          <Typography.H2 id="agents" style={styles.heading}>
            A shared identity for everything your agents encounter.
          </Typography.H2>
          <Typography.P style={styles.sectionLead}>
            Agents discover information across documents, databases, APIs, and other tools. But without a shared
            identity, each agent has to independently figure out whether it's encountered the same thing before.
          </Typography.P>
          <Typography.P style={styles.sectionLead}>
            Spool gives agents a common reference to the people, places, organizations, and other entities they
            discover. As new information arrives, it becomes part of an existing identity rather than another
            disconnected record.
          </Typography.P>
          <Typography.P>One identity, shared across agents and applications.</Typography.P>
          <Button
            variant="outline"
            nativeButton={false}
            render={<Link href="/docs/use-cases" />}
            style={styles.useCasesButton}
          >
            View use cases
          </Button>
        </section>

        <section aria-label="Identity properties" {...stylex.props(styles.section, styles.properties)}>
          {properties.map(({ title, description }) => (
            <div key={title}>
              <Typography.H2 style={styles.heading}>{title}</Typography.H2>
              <Typography.P style={styles.sectionLead}>{description}</Typography.P>
            </div>
          ))}
        </section>

        <section aria-labelledby="start" {...stylex.props(styles.section, styles.columns, styles.terminals)}>
          <div {...stylex.props(styles.terminal)}>
            <Typography.H2 id="start" style={styles.heading}>
              Run it
            </Typography.H2>
            <Typography.P>Install it, make the sample project and start the server on your machine.</Typography.P>
            <CodeBlock wrap>{server}</CodeBlock>
            <Typography.Muted>
              Linux or macOS on Apple silicon. Version {site.version}, {site.licence} licence.
            </Typography.Muted>
          </div>
          <div {...stylex.props(styles.terminal)}>
            <Typography.H2 style={styles.heading}>Then, in a second terminal</Typography.H2>
            <Typography.P>
              Load the rows, match, and extract the companies from the note to reproduce the Acorn Tools demo.
            </Typography.P>
            <CodeBlock wrap>{commands}</CodeBlock>
            <Typography.Muted>
              <Link href="/docs/getting-started" {...stylex.props(styles.link)}>
                Getting started
              </Link>{" "}
              shows what each prints, and the same steps over HTTP and MCP.
            </Typography.Muted>
          </div>
        </section>

        <section aria-label="How you use it" {...stylex.props(styles.section)}>
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
    gap: { default: spacing["3"], [breakpoints.narrow]: spacing["4"] },
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "center",
    marginTop: { default: "2rem", [breakpoints.narrow]: spacing["6"] },
  },
  screen: {
    display: "flex",
    flexDirection: "column",
    gap: "1.75rem",
    marginTop: { default: "4rem", [breakpoints.narrow]: spacing["6"] },
  },
  screenLinks: {
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
    fontSize: "0.875rem",
    gap: spacing["4"],
    justifyContent: "center",
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
  sectionLead: {
    marginTop: spacing["3"],
    color: colors.mutedForeground,
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
  terminals: {
    rowGap: { default: 0, [breakpoints.narrow]: "3rem" },
  },
  terminal: {
    display: { default: "grid", [breakpoints.narrow]: "block" },
    gridRow: { default: "span 4", [breakpoints.narrow]: "auto" },
    gridTemplateRows: { default: "subgrid", [breakpoints.narrow]: "none" },
    minWidth: 0,
  },
  properties: {
    display: "grid",
    gap: "2rem",
    gridTemplateColumns: {
      default: "repeat(3, minmax(0, 1fr))",
      [breakpoints.mobile]: "minmax(0, 1fr)",
    },
  },
  useCasesButton: {
    marginTop: spacing["6"],
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
