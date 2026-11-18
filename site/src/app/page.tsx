import * as stylex from "@stylexjs/stylex";
import Link from "next/link";

import { SiteLinks } from "@/components/site-links";
import { Button } from "@/components/ui/button";
import * as Typography from "@/components/ui/typography";
import { site } from "@/lib/site";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { colors, radius, spacing, typography } from "@/styles/shelf/tokens.stylex";

const points = [
  {
    title: "Rules are files",
    body: "Sources, features and match rules are YAML in a directory you keep in Git. What decided a match is something you can read and review.",
    href: "/docs/project-structure",
  },
  {
    title: "IDs that stay",
    body: "An identity keeps its ID when new records join it. Merges and splits are recorded, so an old ID still leads somewhere.",
    href: "/docs/concepts/identities",
  },
  {
    title: "Every match explained",
    body: "Ask why two records are the same and get the rule that fired and the values it compared. What the rules could not decide waits for a person.",
    href: "/docs/concepts/matching",
  },
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
          </nav>
          <SiteLinks />
        </div>
      </header>

      <main {...stylex.props(styles.main)}>
        <section {...stylex.props(styles.hero)}>
          <Typography.H1 style={styles.title}>
            Know which records are the same thing, and keep knowing it.
          </Typography.H1>
          <Typography.Lead style={styles.lead}>{site.summary}</Typography.Lead>
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

        <section aria-label="What Spool does" {...stylex.props(styles.points)}>
          {points.map((point) => (
            <Link key={point.title} href={point.href} {...stylex.props(styles.point)}>
              <Typography.Large>{point.title}</Typography.Large>
              <Typography.Muted style={styles.pointBody}>{point.body}</Typography.Muted>
            </Link>
          ))}
        </section>

        <Typography.Muted style={styles.note}>
          Spool is early. Nothing is published yet, so today you build it from source. The{" "}
          <Link href="/docs/limits" {...stylex.props(styles.link)}>
            limits page
          </Link>{" "}
          says what it does not do.
        </Typography.Muted>
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
    maxWidth: "44rem",
  },
  title: {
    fontSize: {
      default: "3rem",
      [breakpoints.mobile]: "2.25rem",
    },
    lineHeight: {
      default: "3.25rem",
      [breakpoints.mobile]: "2.5rem",
    },
  },
  lead: {
    marginTop: spacing["6"],
  },
  actions: {
    gap: spacing["3"],
    display: "flex",
    flexWrap: "wrap",
    marginTop: "2rem",
  },
  points: {
    gap: spacing["4"],
    display: "grid",
    gridTemplateColumns: {
      default: "repeat(3, 1fr)",
      [breakpoints.narrow]: "1fr",
    },
    marginTop: "5rem",
  },
  point: {
    padding: spacing["6"],
    borderColor: {
      default: colors.border,
      ":hover": colors.ring,
    },
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    textDecoration: "none",
    display: "block",
  },
  pointBody: {
    marginTop: spacing["2"],
  },
  note: {
    marginTop: "3rem",
  },
  link: {
    color: colors.foreground,
    textDecorationLine: "underline",
    textUnderlineOffset: 4,
  },
});
