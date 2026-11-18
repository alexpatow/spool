import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { ImageResponse } from "next/og";

import { Wordmark } from "@/components/brand";
import { cardSize } from "@/lib/sharing";
import { siteOrigin } from "@/lib/site";

/**
 * The light theme's colours, from `styles/shelf/tokens.stylex.ts`. A card is
 * drawn when the site is built, outside the page, so it cannot read them.
 */
const colors = {
  background: "#fbfaf8",
  foreground: "#1a1917",
  mutedForeground: "#6b6862",
  faint: "#9d9990",
  lineStrong: "#cfcbc2",
  thread: "#a4442f",
};

/**
 * The typefaces of the site, as static TrueType files kept in `fonts/` under
 * the licences beside them. The card renderer cannot read the WOFF2 the site
 * serves, and fails on Commit Mono's WOFF and OTF.
 */
const typefaces = [
  { name: "Newsreader", weight: 400, file: "newsreader-display-400.ttf" },
  { name: "Hanken Grotesk", weight: 400, file: "hanken-grotesk-400.ttf" },
  { name: "Hanken Grotesk", weight: 500, file: "hanken-grotesk-500.ttf" },
  { name: "Commit Mono", weight: 400, file: "commit-mono-400.ttf" },
] as const;

function fonts() {
  const here = join(process.cwd(), "src/og/fonts");
  return Promise.all(
    typefaces.map(async ({ name, weight, file }) => ({
      name,
      weight,
      style: "normal" as const,
      data: await readFile(join(here, file)),
    })),
  );
}

interface Card {
  /** Where the page sits, such as `Docs / Concepts`. */
  section: string;
  title: string;
  description: string;
  /** The page's address, relative to the site, which the card prints. */
  path: string;
}

/** A page's card for link previews: the wordmark, the page's title and description, and its address. */
export async function card({ section, title, description, path }: Card): Promise<ImageResponse> {
  const address = `${siteOrigin().replace(/^https?:\/\//, "")}${path === "/" ? "" : path}`;
  const long = title.length > 34;

  return new ImageResponse(
    <div
      style={{
        width: "100%",
        height: "100%",
        display: "flex",
        flexDirection: "column",
        justifyContent: "space-between",
        padding: "64px 80px 60px",
        backgroundColor: colors.background,
        color: colors.foreground,
        fontFamily: "Hanken Grotesk",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <Wordmark width={149} height={50} style={{ color: colors.foreground }} />
        <div style={{ fontFamily: "Commit Mono", fontSize: 22, color: colors.mutedForeground }}>{section}</div>
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 28 }}>
        <div
          style={{
            fontFamily: "Newsreader",
            fontSize: long ? 76 : 92,
            lineHeight: 1.04,
            letterSpacing: "-0.022em",
            maxWidth: 1000,
          }}
        >
          {title}
        </div>
        <div
          style={{
            display: "block",
            fontSize: 30,
            lineHeight: 1.4,
            color: colors.mutedForeground,
            maxWidth: 1000,
            lineClamp: 3,
          }}
        >
          {description}
        </div>
      </div>

      <div style={{ display: "flex", alignItems: "center", gap: 24 }}>
        <div style={{ width: 56, height: 3, backgroundColor: colors.thread }} />
        <div style={{ fontFamily: "Commit Mono", fontSize: 22, color: colors.faint }}>{address}</div>
        <div style={{ flexGrow: 1, height: 1, backgroundColor: colors.lineStrong }} />
      </div>
    </div>,
    { ...cardSize, fonts: await fonts() },
  );
}
