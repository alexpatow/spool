import * as stylex from "@stylexjs/stylex";
import type { Metadata } from "next";
import Link from "next/link";

import { Explainer } from "@/components/explainer/scenes";
import { SiteHeader } from "@/components/site-header";
import { Button } from "@/components/ui/button";
import * as Typography from "@/components/ui/typography";
import { explainer } from "@/lib/explainer";
import { sharing } from "@/lib/sharing";
import { site } from "@/lib/site";
import { layout, breakpoints } from "@/styles/layout.stylex";
import { colors, display, linen, spacing, typography } from "@/styles/shelf/tokens.stylex";

export const metadata: Metadata = {
  ...sharing({
    title: `${explainer.title} | ${site.name}`,
    description: explainer.description,
    path: explainer.path,
    image: explainer.card,
    imageAlt: `${explainer.title} ${explainer.description}`,
    type: "article",
  }),
  title: explainer.title,
  description: explainer.description,
};

const terms = [
  { term: "Record", says: "One thing a source said: a row, or a mention found in text. Kept as it arrived." },
  { term: "Mention", says: "A place in a document that names a thing, found by an extractor." },
  { term: "Judgement", says: "The answer for one pair of records: match or no_match." },
  { term: "Identity", says: "One real-world thing: an ID, and the records placed in it." },
  { term: "Changeset", says: "Every change to identities, in a log, with an author, a reason and an undo." },
];

export default function Explained() {
  return (
    <div {...stylex.props(styles.page)}>
      <SiteHeader />
      <main {...stylex.props(styles.main)}>
        <section {...stylex.props(styles.hero)}>
          <p {...stylex.props(styles.eyebrow)}>Explained</p>
          <Typography.H1 style={styles.title}>{explainer.title}</Typography.H1>
          <Typography.P style={styles.lead}>
            Your data names the same companies in different ways. Spool finds them in text, works out
            which records are the same company, and gives each one an ID that stays. Scroll, and
            watch it happen to the sample data in Getting started.
          </Typography.P>
          <div {...stylex.props(styles.cue)} aria-hidden="true">
            <span {...stylex.props(styles.cueLine)} />
          </div>
        </section>

        <Explainer />

        <section aria-labelledby="run-it" {...stylex.props(styles.end)}>
          <h2 id="run-it" {...stylex.props(styles.endTitle)}>
            Now run it
          </h2>
          <Typography.P style={styles.endLead}>
            Getting started does all of this on your machine, with the same files, in about five
            minutes.
          </Typography.P>
          <dl {...stylex.props(styles.terms)}>
            {terms.map(({ term, says }) => (
              <div key={term} {...stylex.props(styles.termRow)}>
                <dt {...stylex.props(styles.term)}>{term}</dt>
                <dd {...stylex.props(styles.says)}>{says}</dd>
              </div>
            ))}
          </dl>
          <div {...stylex.props(styles.actions)}>
            <Button size="lg" nativeButton={false} render={<Link href="/docs/getting-started" />}>
              Get started
            </Button>
            <Button
              variant="outline"
              size="lg"
              nativeButton={false}
              render={<Link href="/docs/concepts/records-and-sources" />}
            >
              Read the concepts
            </Button>
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
  main: {
    marginInline: "auto",
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
    paddingBottom: { default: "4rem", [breakpoints.mobile]: "2rem" },
    paddingTop: { default: "6rem", [breakpoints.mobile]: "3rem" },
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    maxWidth: "46rem",
    minHeight: { default: "calc(100svh - 3.5rem)", [breakpoints.mobile]: "auto" },
    boxSizing: "border-box",
    justifyContent: "center",
    textAlign: "center",
  },
  eyebrow: {
    margin: 0,
    marginBottom: spacing["6"],
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.75rem",
    letterSpacing: "0.08em",
    textTransform: "uppercase",
  },
  title: {
    fontSize: {
      default: "3.75rem",
      [breakpoints.mobile]: "2.375rem",
    },
    lineHeight: 1.04,
    textWrap: "balance",
  },
  lead: {
    marginTop: spacing["6"],
    maxWidth: "36rem",
    color: colors.mutedForeground,
    fontSize: "1.125rem",
    lineHeight: "1.75rem",
    textWrap: "pretty",
  },
  cue: {
    marginTop: "3rem",
    backgroundColor: colors.border,
    height: "3.5rem",
    overflow: "hidden",
    position: "relative",
    width: 1,
  },
  cueLine: {
    animationDuration: "2.2s",
    animationIterationCount: { default: "infinite", "@media (prefers-reduced-motion: reduce)": 0 },
    animationName: stylex.keyframes({
      "0%": { transform: "translateY(-100%)" },
      "60%, 100%": { transform: "translateY(100%)" },
    }),
    animationTimingFunction: "cubic-bezier(0.65, 0, 0.35, 1)",
    backgroundColor: linen.thread,
    display: "block",
    height: "100%",
    width: "100%",
  },
  end: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    marginInline: "auto",
    maxWidth: "46rem",
    paddingBottom: "6rem",
    paddingTop: "4rem",
  },
  endTitle: {
    margin: 0,
    color: colors.foreground,
    fontFamily: display.fontFamily,
    fontSize: { default: "2.25rem", [breakpoints.mobile]: "1.75rem" },
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: 1.1,
  },
  endLead: {
    marginTop: spacing["4"],
    color: colors.mutedForeground,
    fontSize: "1.0625rem",
  },
  terms: {
    margin: 0,
    marginTop: "2rem",
  },
  termRow: {
    gap: spacing["4"],
    paddingBlock: spacing["3"],
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "grid",
    gridTemplateColumns: {
      default: "8rem minmax(0, 1fr)",
      [breakpoints.mobile]: "minmax(0, 1fr)",
    },
    rowGap: spacing["1"],
  },
  term: {
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
  },
  says: {
    margin: 0,
    color: colors.foreground,
    fontSize: "0.9375rem",
    lineHeight: "1.5rem",
  },
  actions: {
    gap: spacing["3"],
    display: "flex",
    flexWrap: "wrap",
    marginTop: "2rem",
  },
});
