import * as stylex from "@stylexjs/stylex";
import Link from "next/link";

import { CodeBlock } from "@/components/code-block";
import { SiteLinks } from "@/components/site-links";
import { Button } from "@/components/ui/button";
import * as Typography from "@/components/ui/typography";
import { site } from "@/lib/site";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { colors, spacing, typography } from "@/styles/shelf/tokens.stylex";

const start = `git clone ${site.repository}.git
cd spool
cargo build --release --locked -p spool-cli
export PATH="$PWD/target/release:$PATH"
spool init ../companies --entity company --source crm
cd ../companies
spool dev`;

const rule = `-- entities/company/rules/match/same_name.sql
l.name_key = r.name_key`;

const firstBatch = `$ curl -s -X POST http://127.0.0.1:7765/v1/ingest -d @batch-1.json
$ curl -s -X POST http://127.0.0.1:7765/v1/runs -d '{"entity_type": "company"}'
$ spool show records
source and key  identity
crm:c-1         01a1ef24-76bc-73d9-9c8c-469e735d8cdd
crm:c-2         01a1ef24-76bc-73d9-9c8c-469e735d8cdd
crm:c-3         01a1ef24-76bc-73d9-9c8c-469f8e792cfb`;

const secondBatch = `$ curl -s -X POST http://127.0.0.1:7765/v1/ingest -d @batch-2.json
$ curl -s -X POST http://127.0.0.1:7765/v1/runs -d '{"entity_type": "company"}'
$ spool show records
source and key  identity
crm:c-5         01a1ef24-7706-701a-a707-bebd05a4f9e6
crm:c-1         01a1ef24-76bc-73d9-9c8c-469e735d8cdd
crm:c-4         01a1ef24-76bc-73d9-9c8c-469e735d8cdd
crm:c-2         01a1ef24-76bc-73d9-9c8c-469e735d8cdd
crm:c-3         01a1ef24-76bc-73d9-9c8c-469f8e792cfb`;

const does = [
  "Takes rows from several sources and keeps each one as it arrived.",
  "Matches them with rules you write in SQL, kept as files in Git.",
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
            {site.name}
          </Link>
          <nav aria-label="Site" {...stylex.props(styles.nav)}>
            <Button variant="ghost" size="sm" nativeButton={false} render={<Link href="/docs" />}>
              Docs
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
          <Typography.H1 style={styles.title}>{site.summary}</Typography.H1>
          <Typography.P style={styles.lead}>{site.problem}</Typography.P>
          <CodeBlock>{start}</CodeBlock>
          <Typography.Muted>
            Needs Rust 1.89 or later and a C compiler. Version {site.version}, with no release yet, so the first
            step is a build from source. {site.licence} licence.
          </Typography.Muted>
          <div {...stylex.props(styles.actions)}>
            <Button size="lg" nativeButton={false} render={<Link href="/docs/getting-started" />}>
              Getting started
            </Button>
            <Button
              variant="outline"
              size="lg"
              nativeButton={false}
              render={<Link href="/docs/limits" />}
            >
              Limits: what it does not do
            </Button>
          </div>
        </section>

        <section aria-labelledby="sample" {...stylex.props(styles.section)}>
          <Typography.H2 id="sample" style={styles.heading}>
            What it looks like
          </Typography.H2>
          <div {...stylex.props(styles.steps)}>
            <div>
              <Typography.P style={styles.step}>
                A match rule is a file that holds one SQL condition over two records.
              </Typography.P>
              <CodeBlock>{rule}</CodeBlock>
            </div>
            <div>
              <Typography.P style={styles.step}>
                Load three rows and run. Two of them are "Acorn Tools" and "ACORN TOOLS", so they
                share an identity.
              </Typography.P>
              <CodeBlock copy={false}>{firstBatch}</CodeBlock>
            </div>
            <div>
              <Typography.P style={styles.step}>
                Load two more. "Acorn tools" joins the identity that exists, and no ID that was handed
                out before changes.
              </Typography.P>
              <CodeBlock copy={false}>{secondBatch}</CodeBlock>
            </div>
          </div>
          <Typography.Muted style={styles.caption}>
            Shortened from{" "}
            <Link href="/docs/getting-started" {...stylex.props(styles.link)}>
              Getting started
            </Link>
            , which has every command and its full output.
          </Typography.Muted>
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
    gap: spacing["4"],
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
    fontSize: typography.fontSizeLg,
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.02em",
  },
  nav: {
    display: "flex",
    flexGrow: 1,
  },
  main: {
    marginInline: "auto",
    paddingBlock: {
      default: "6rem",
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
    maxWidth: "46rem",
  },
  title: {
    fontSize: {
      default: "2rem",
      [breakpoints.mobile]: "1.5rem",
    },
    lineHeight: {
      default: "2.5rem",
      [breakpoints.mobile]: "2rem",
    },
  },
  lead: {
    marginTop: spacing["6"],
  },
  actions: {
    gap: spacing["3"],
    display: "flex",
    flexWrap: "wrap",
    marginTop: spacing["6"],
  },
  section: {
    marginTop: "4.5rem",
  },
  heading: {
    borderBottomWidth: 0,
    fontSize: "1.25rem",
    lineHeight: "1.75rem",
    paddingBottom: 0,
  },
  steps: {
    maxWidth: "46rem",
  },
  step: {
    marginBottom: 0,
    marginTop: spacing["6"],
  },
  caption: {
    marginTop: spacing["4"],
  },
  columns: {
    gap: "3rem",
    display: "grid",
    gridTemplateColumns: {
      default: "1fr 1fr",
      [breakpoints.narrow]: "1fr",
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
    fontSize: typography.fontSizeBase,
    lineHeight: "1.75rem",
  },
  link: {
    color: colors.foreground,
    textDecorationLine: "underline",
    textUnderlineOffset: 4,
  },
});
