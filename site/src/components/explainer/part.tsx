import * as stylex from "@stylexjs/stylex";
import type { CSSProperties, ReactNode } from "react";

import { colors, linen, typography } from "spool-ui/tokens.stylex";

/** Custom properties for an element's inline style. */
export function vars(values: Record<string, string | number>): CSSProperties {
  return values as CSSProperties;
}

/**
 * When a part shows, moves and is lit, by step. Every value is worked out by
 * the browser from `--step`, which is unset in the still picture, and the
 * fallback of 99 then draws the part as it is after the last step.
 *
 *   at       the step it appears at
 *   until    the step it is gone at
 *   from     where it comes from as it appears, relative to where it is
 *   move     where it goes, relative to where it is, and at which step
 *   lit      the steps it is drawn in the thread's colour
 */
export interface Timing {
  at?: number;
  until?: number;
  from?: { x?: number; y?: number; scale?: number; rotate?: number };
  move?: { x?: number; y?: number; at: number; rotate?: number };
  lit?: { at: number; until?: number };
  /** Milliseconds after the step before it starts, so that parts can follow one another. */
  delay?: number;
  /** Milliseconds it takes. */
  duration?: number;
}

export function timing({ at, until, from, move, lit, delay, duration }: Timing) {
  return vars({
    "--at": at ?? 0,
    "--until": until ?? 999,
    "--fx": from?.x ?? 0,
    "--fy": from?.y ?? 0,
    "--fs": from?.scale ?? 1,
    "--fr": from?.rotate ?? 0,
    "--mx": move?.x ?? 0,
    "--my": move?.y ?? 0,
    "--mr": move?.rotate ?? 0,
    "--mat": move?.at ?? 999,
    "--lat": lit?.at ?? 999,
    "--luntil": lit?.until ?? 999,
    "--delay": delay ?? 0,
    "--duration": duration ?? 800,
  });
}

export function Part({
  x,
  y,
  width,
  height,
  rotate = 0,
  style,
  children,
  ...when
}: Timing & {
  x: number;
  y: number;
  width?: number;
  height?: number;
  rotate?: number;
  style?: stylex.StyleXStyles;
  children?: ReactNode;
}) {
  return (
    <div
      {...stylex.props(parts.part, style)}
      style={{
        ...timing(when),
        ...vars({ "--r": rotate }),
        left: x,
        top: y,
        ...(width !== undefined && { width }),
        ...(height !== undefined && { height }),
      }}
    >
      {children}
    </div>
  );
}

/** 1 from the step a part appears at, 0 before. */
const IN = "clamp(0, calc(var(--step, 99) - var(--at) + 1), 1)";
/** 1 before the step a part is gone at, 0 from it. */
const OUT = "clamp(0, calc(var(--until) - var(--step, 99)), 1)";
/** 1 from the step a part moves at. */
const MOVED = "clamp(0, calc(var(--step, 99) - var(--mat) + 1), 1)";
/** 1 while a part is lit. */
const LIT =
  "min(clamp(0, calc(var(--step, 99) - var(--lat) + 1), 1), clamp(0, calc(var(--luntil) - var(--step, 99)), 1))";

const easing = "cubic-bezier(0.22, 1, 0.36, 1)";

export const parts = stylex.create({
  part: {
    boxSizing: "border-box",
    position: "absolute",
    opacity: `min(${IN}, ${OUT})`,
    transform: `translate(calc((var(--fx) * (1 - ${IN}) + var(--mx) * ${MOVED}) * 1px), calc((var(--fy) * (1 - ${IN}) + var(--my) * ${MOVED}) * 1px)) rotate(calc((var(--r) + var(--fr) * (1 - ${IN}) + var(--mr) * ${MOVED}) * 1deg)) scale(calc(1 - (1 - var(--fs)) * (1 - ${IN})))`,
    transitionDelay: "calc(var(--motion, 0) * var(--delay) * 1ms)",
    transitionDuration: "calc(var(--motion, 0) * var(--duration) * 1ms)",
    transitionProperty: "opacity, transform, color, background-color, border-color",
    transitionTimingFunction: easing,
  },
  /** Text that turns the thread's colour while lit. */
  litText: {
    color: `color-mix(in oklab, ${linen.thread} calc(${LIT} * 100%), ${colors.mutedForeground})`,
  },
  litStrong: {
    color: `color-mix(in oklab, ${linen.thread} calc(${LIT} * 100%), ${colors.foreground})`,
  },
  litWash: {
    backgroundColor: `color-mix(in oklab, ${linen.threadWash} calc(${LIT} * 100%), transparent)`,
  },
  litBorder: {
    borderColor: `color-mix(in oklab, ${linen.thread} calc(${LIT} * 100%), ${colors.border})`,
  },
  /** A line drawn from one end to the other as it appears. Its path needs `pathLength={1}`. */
  stroke: {
    fill: "none",
    opacity: OUT,
    strokeDasharray: 1,
    strokeDashoffset: `calc(1 - ${IN})`,
    transitionDelay: "calc(var(--motion, 0) * var(--delay) * 1ms)",
    transitionDuration: "calc(var(--motion, 0) * var(--duration) * 1ms)",
    transitionProperty: "opacity, stroke-dashoffset",
    transitionTimingFunction: easing,
  },
  card: {
    borderColor: colors.border,
    borderStyle: "solid",
    borderWidth: 1,
    backgroundColor: colors.card,
  },
  mono: {
    fontFamily: typography.fontFamilyMono,
  },
});
