import * as stylex from "@stylexjs/stylex";

import { Mark } from "spool-ui/brand";
import { vars } from "@/components/explainer/part";
import { identities, records } from "@/lib/sample";
import { colors, display, linen, typography } from "spool-ui/tokens.stylex";

const acorn = identities[0];
const spellings = [...new Set(records.filter((record) => record.identity === 0).map((record) => record.name))];

/** Where each spelling starts, around the identity it ends in: x and y in pixels, and a turn in degrees. */
const scatter = [
  [-210, -34, -5],
  [170, -42, 4],
  [-150, 40, 3],
  [205, 34, -4],
] as const;

/**
 * The page's first picture, played once: the four ways the sample data
 * writes Acorn Tools drift together and become one identity. Without
 * motion it is the end of that, the identity alone.
 */
export function Hero() {
  return (
    <div {...stylex.props(styles.stage)} aria-hidden="true">
      {spellings.map((name, index) => {
        const [x, y, turn] = scatter[index % scatter.length] ?? [0, 0, 0];
        return (
          <span
            key={name}
            {...stylex.props(styles.spelling)}
            style={vars({ "--x": `${x}px`, "--y": `${y}px`, "--turn": `${turn}deg`, "--i": index })}
          >
            {name}
          </span>
        );
      })}
      <span {...stylex.props(styles.identity)}>
        <span {...stylex.props(styles.name)}>
          <Mark width={18} height={18} {...stylex.props(styles.mark)} />
          {acorn.name}
        </span>
        <span {...stylex.props(styles.id)}>{acorn.id}</span>
      </span>
    </div>
  );
}

const gather = stylex.keyframes({
  "0%": { opacity: 0, transform: "translate(calc(var(--x) * var(--spread)), calc(var(--y) + 14px)) rotate(var(--turn))" },
  "18%": { opacity: 1, transform: "translate(calc(var(--x) * var(--spread)), var(--y)) rotate(var(--turn))" },
  "52%": { opacity: 1, transform: "translate(calc(var(--x) * var(--spread)), var(--y)) rotate(var(--turn))" },
  "82%": { opacity: 1, transform: "translate(0, 0) rotate(0deg) scale(0.92)" },
  "100%": { opacity: 0, transform: "translate(0, 0) rotate(0deg) scale(0.86)" },
});

const arrive = stylex.keyframes({
  "0%, 76%": { opacity: 0, transform: "scale(0.9)" },
  "100%": { opacity: 1, transform: "scale(1)" },
});

const motion = "@media (prefers-reduced-motion: no-preference)";

const styles = stylex.create({
  stage: {
    "--spread": { default: 1, "@media (width < 40rem)": 0.42 },
    alignItems: "center",
    display: "grid",
    justifyItems: "center",
    marginTop: "3rem",
    height: "8.5rem",
    width: "100%",
  },
  spelling: {
    borderColor: colors.border,
    borderStyle: "solid",
    borderWidth: 1,
    backgroundColor: colors.card,
    color: colors.foreground,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
    gridArea: "1 / 1",
    lineHeight: "1.75rem",
    paddingInline: "0.75rem",
    whiteSpace: "nowrap",
    opacity: 0,
    animationDelay: "calc(300ms + var(--i) * 90ms)",
    animationDuration: "2600ms",
    animationFillMode: "both",
    animationName: { default: "none", [motion]: gather },
    animationTimingFunction: "cubic-bezier(0.65, 0, 0.35, 1)",
  },
  identity: {
    gap: 2,
    paddingBlock: "0.625rem",
    paddingInline: "1rem",
    alignItems: "center",
    backgroundColor: `color-mix(in oklab, ${linen.threadWash} 70%, ${colors.card})`,
    borderColor: `color-mix(in oklab, ${linen.thread} 45%, ${colors.border})`,
    borderStyle: "solid",
    borderWidth: 1,
    display: "flex",
    flexDirection: "column",
    gridArea: "1 / 1",
    animationDelay: "570ms",
    animationDuration: "2600ms",
    animationFillMode: "both",
    animationName: { default: "none", [motion]: arrive },
    animationTimingFunction: "cubic-bezier(0.22, 1, 0.36, 1)",
  },
  name: {
    gap: 8,
    alignItems: "center",
    color: colors.foreground,
    display: "flex",
    fontFamily: display.fontFamily,
    fontSize: "1.375rem",
    fontWeight: linen.displayWeight,
    lineHeight: "1.75rem",
  },
  mark: {
    color: linen.thread,
  },
  id: {
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: { default: "0.75rem", "@media (width < 30rem)": "0.625rem" },
  },
});
