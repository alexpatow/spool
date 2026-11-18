import * as stylex from "@stylexjs/stylex";
import Link from "next/link";

import { Wordmark } from "@/components/brand";
import { CodeBlock } from "@/components/code-block";
import { FirstScreen } from "@/components/first-screen";
import { SiteLinks } from "@/components/site-links";
import { Button } from "@/components/ui/button";
import * as Typography from "@/components/ui/typography";
import { site } from "@/lib/site";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { colors, linen, spacing, typography } from "@/styles/shelf/tokens.stylex";

const server = `curl -fsSL ${site.repository}/releases/latest/download/install.sh | sh
spool init companies
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
  "Takes rows from several sources and keeps each one as it arrived.",
  "Finds records in text with an extractor, which is code in Python or TypeScript, and keeps where in the text each one was found.",
  "Matches records with rules you write in SQL, kept as files in Git.",
  "Gives each real-world thing an ID that later runs and better rules do not change.",
  "Records every merge, split and correction in a log, with an author, a reason and an undo.",
  "Says why two records were matched, and leaves what it cannot decide for a person or an agent.",
];

const notFor = [
  "Deduplicating one file once. A script or a library is less to run.",
  "Answering per record, in the request, whether a row is a duplicate. Matching runs in batches.",
  "Matching with no rules of your own. There is no trained model inside.",
  "Building a merged golden record, or writing corrections back to a source.",
  "Many tenants, many writers or many machines. One server owns one store.",
];

export default function Home() {
  return (
    <div {...stylex.props(styles.page)}>
      <header {...stylex.props(styles.header)}>
        <div {...stylex.props(styles.headerInner)}>
          <Link href="/" {...stylex.props(styles.wordmark)}>
            <Wordmark />
          </Link>
          <nav aria-label="Site" {...stylex.props(styles.nav)}>
            <Button variant="ghost" size="sm" nativeButton={false} render={<Link href="/docs" />}>
              Docs
            </Button>
            <Button
              variant="ghost"
              size="sm"
              nativeButton={false}
              render={<Link href="/docs/getting-started" />}
            >
              Getting started
            </Button>
            <Button
              variant="ghost"
              size="sm"
              nativeButton={false}
              render={<Link href="/docs/limits" />}
            >
              Limits
            </Button>
          </nav>
          <SiteLinks />
        </div>
      </header>

      <main {...stylex.props(styles.main)}>
        <section {...stylex.props(styles.hero)}>
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
              render={<Link href="/docs/limits" />}
            >
              What it is not for
            </Button>
          </div>
        </section>

        <section aria-label="The sample project" {...stylex.props(styles.screen)}>
          <FirstScreen />
          <Typography.Muted style={styles.screenCaption}>
            The sample project at the end of{" "}
            <Link href="/docs/getting-started" {...stylex.props(styles.link)}>
              Getting started
            </Link>
            : nine rows from a CRM and one call note. "Acorn Tools Ltd" in the note joined the
            identity that four CRM rows already had, and its ID did not change.
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
              Load the rows, match them, and get the companies out of the note. These seven commands
              give the picture above.
            </Typography.P>
            <CodeBlock>{commands}</CodeBlock>
            <Typography.Muted>
              <Link href="/docs/getting-started" {...stylex.props(styles.link)}>
                Getting started
              </Link>{" "}
              has what each one prints, and the same steps over HTTP and MCP.
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
            <Typography.H2 style={styles.heading}>What it is not meant for</Typography.H2>
            <ul {...stylex.props(styles.list)}>
              {notFor.map((item) => (
                <li key={item} {...stylex.props(styles.item)}>
                  {item}
                </li>
              ))}
            </ul>
            <Typography.Muted style={styles.caption}>
              <Link href="/docs/limits" {...stylex.props(styles.link)}>
                Limits
              </Link>{" "}
              has the full list, with what it does not do yet.
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
  header: {
    borderBottomColor: colors.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
  },
  headerInner: {
    gap: spacing["6"],
    marginInline: "auto",
    paddingInline: {
      default: "2.5rem",
      [breakpoints.mobile]: spacing["4"],
    },
    alignItems: "center",
    boxSizing: "border-box",
    display: "flex",
    height: layout.headerHeight,
    maxWidth: layout.pageWidth,
  },
  wordmark: {
    textDecoration: "none",
    color: colors.foreground,
    display: "flex",
  },
  nav: {
    display: {
      default: "flex",
      [breakpoints.mobile]: "none",
    },
    flexGrow: 1,
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
