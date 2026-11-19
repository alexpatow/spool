import * as stylex from "@stylexjs/stylex";
import { type ComponentProps, type ReactNode, isValidElement } from "react";

import { CopyButton } from "@/components/ui/copy-button";
import { colors, radius, spacing, typography } from "@/styles/shelf/tokens.stylex";

/** The text of a highlighted block, which is what the copy button copies. */
function textOf(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  return "";
}

/**
 * A fenced code block. The colours were worked out when the site was built:
 * every token carries a light and a dark one, and `globals.css` picks by
 * theme. A block written with `title="…"` shows it as a file name. A block
 * that is a transcript, not something to paste, is given `copy={false}`.
 */
export function CodeBlock({
  title,
  children,
  copy = true,
  wrap = false,
}: ComponentProps<"pre"> & { copy?: boolean; wrap?: boolean }) {
  // MDX wraps the lines in a `code` element; render its lines in ours, so the
  // inline-code style does not apply.
  const lines = isValidElement<{ children?: ReactNode }>(children)
    ? children.props.children
    : children;
  const text = textOf(lines).replace(/\n$/, "");

  return (
    <figure {...stylex.props(styles.figure)}>
      {title ? <figcaption {...stylex.props(styles.title)}>{title}</figcaption> : null}
      <div {...stylex.props(styles.body)}>
        <pre className={`shiki ${stylex.props(styles.pre, wrap && styles.wrap).className ?? ""}`} tabIndex={0}>
          <code {...stylex.props(styles.code)}>{lines}</code>
        </pre>
        {copy ? (
          <div {...stylex.props(styles.copy)}>
            <CopyButton text={text} />
          </div>
        ) : null}
      </div>
    </figure>
  );
}

const styles = stylex.create({
  figure: {
    marginBlock: spacing["6"],
    marginInline: 0,
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    overflow: "hidden",
    backgroundColor: colors.card,
  },
  title: {
    paddingBlock: spacing["2"],
    paddingInline: spacing["4"],
    borderBottomColor: colors.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    color: colors.mutedForeground,
    fontFamily: typography.fontFamilyMono,
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
  },
  body: {
    position: "relative",
  },
  pre: {
    margin: 0,
    outline: {
      default: "none",
      ":focus-visible": `2px solid ${colors.ring}`,
    },
    outlineOffset: -2,
    paddingBlock: spacing["4"],
    // Room on the right so the first line never runs under the copy button.
    paddingInlineEnd: "3rem",
    paddingInlineStart: spacing["4"],
    color: colors.foreground,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
    lineHeight: "1.5rem",
    overflowX: "auto",
    tabSize: 2,
  },
  code: {
    fontFamily: "inherit",
  },
  wrap: {
    overflowWrap: "anywhere",
    whiteSpace: "pre-wrap",
  },
  copy: {
    backgroundColor: colors.card,
    position: "absolute",
    insetInlineEnd: spacing["2"],
    top: spacing["2"],
  },
});
