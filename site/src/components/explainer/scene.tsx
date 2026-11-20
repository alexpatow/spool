"use client";

import * as stylex from "@stylexjs/stylex";
import { type ReactNode, useEffect, useRef, useState } from "react";

import { vars } from "@/components/explainer/part";
import { breakpoints } from "@/styles/layout.stylex";
import { colors, display, linen, spacing, typography } from "spool-ui/tokens.stylex";

/** The size every scene's picture is drawn at, in pixels, before it is scaled to fit. */
export const WIDTH = 420;
export const HEIGHT = 560;

/** How far the page scrolls for each step of a scene, beyond the screen the scene is pinned for. */
const PER_STEP = "75svh";

/**
 * A scene: captions on one side and a picture on the other, pinned while the
 * page scrolls through its steps.
 *
 * The step is written to the custom property `--step` on the section, and
 * the picture's parts read it in their styles, so a step changes nothing but
 * one property and the browser animates the rest. With `--step` unset, which
 * is the page before it runs, without JavaScript and for a reader who asked
 * for less motion, every part is drawn as it is after the last step, and the
 * captions are one after another.
 */
export function Scene({
  id,
  number,
  name,
  captions,
  children,
}: {
  id: string;
  number: number;
  name: string;
  captions: ReactNode[];
  children: ReactNode;
}) {
  const section = useRef<HTMLElement>(null);
  const live = useMotion();
  const steps = captions.length;

  useEffect(() => {
    const element = section.current;
    if (!live || !element) return;
    let frame = 0;
    let shown = -1;
    const measure = () => {
      frame = 0;
      const box = element.getBoundingClientRect();
      const room = box.height - window.innerHeight;
      const progress = room > 0 ? Math.min(1, Math.max(0, -box.top / room)) : 0;
      const step = Math.min(steps - 1, Math.floor(progress * steps));
      if (step === shown) return;
      shown = step;
      element.style.setProperty("--step", String(step));
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(measure);
    };
    measure();
    // The first step is drawn at once. Transitions start after it, so that
    // nothing is seen moving from the still picture to the first step.
    let ready = requestAnimationFrame(() => {
      ready = requestAnimationFrame(() => element.style.setProperty("--motion", "1"));
    });
    addEventListener("scroll", schedule, { passive: true });
    addEventListener("resize", schedule);
    return () => {
      cancelAnimationFrame(frame);
      cancelAnimationFrame(ready);
      removeEventListener("scroll", schedule);
      removeEventListener("resize", schedule);
      element.style.removeProperty("--step");
      element.style.removeProperty("--motion");
    };
  }, [live, steps]);

  return (
    <section
      ref={section}
      id={id}
      aria-labelledby={`${id}-name`}
      {...stylex.props(styles.section, live && styles.sectionLive)}
      style={vars({ "--steps": steps })}
    >
      <div {...stylex.props(styles.stage, live && styles.stageLive)}>
        <div {...stylex.props(styles.words)}>
          <h2 id={`${id}-name`} {...stylex.props(styles.name)}>
            <span {...stylex.props(styles.number)}>{String(number).padStart(2, "0")}</span>
            {name}
          </h2>
          <div {...stylex.props(styles.captions, live && styles.captionsLive)}>
            {captions.map((caption, index) => (
              <p
                key={index}
                {...stylex.props(styles.caption, live && styles.captionLive)}
                style={vars({ "--k": index })}
              >
                {caption}
              </p>
            ))}
          </div>
          {live ? (
            <div {...stylex.props(styles.dots)} aria-hidden="true">
              {captions.map((_, index) => (
                <span key={index} {...stylex.props(styles.dot)} style={vars({ "--k": index })} />
              ))}
            </div>
          ) : null}
        </div>
        <Picture live={live}>{children}</Picture>
      </div>
    </section>
  );
}

/** Whether the page may move: false until it runs, and for a reader who asked for less motion. */
function useMotion(): boolean {
  const [live, setLive] = useState(false);
  useEffect(() => {
    const query = matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setLive(!query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  return live;
}

/** The picture, drawn at `WIDTH` by `HEIGHT` and scaled to the room it has. */
function Picture({ live, children }: { live: boolean; children: ReactNode }) {
  const outer = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const element = outer.current;
    if (!element) return;
    const fit = () => {
      const width = element.clientWidth / WIDTH;
      const height = live ? element.clientHeight / HEIGHT : Infinity;
      element.style.setProperty("--scale", String(Math.min(width, height, 1.35)));
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => observer.disconnect();
  }, [live]);

  return (
    <div ref={outer} aria-hidden="true" {...stylex.props(styles.picture, live && styles.pictureLive)}>
      <div {...stylex.props(styles.canvas)}>{children}</div>
    </div>
  );
}

const styles = stylex.create({
  section: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    paddingBlock: "3rem",
  },
  sectionLive: {
    borderTopWidth: 0,
    paddingBlock: 0,
    height: `calc(var(--steps) * ${PER_STEP} + 100svh)`,
  },
  stage: {
    gap: { default: "3rem", [breakpoints.mobile]: spacing["6"] },
    display: "grid",
    gridTemplateColumns: {
      default: "minmax(0, 1fr) minmax(0, 1.15fr)",
      [breakpoints.mobile]: "minmax(0, 1fr)",
    },
    alignItems: "center",
  },
  stageLive: {
    gap: { default: "3rem", [breakpoints.mobile]: spacing["3"] },
    paddingBlock: { default: "4.5rem 3rem", [breakpoints.mobile]: "1.25rem 1rem" },
    boxSizing: "border-box",
    gridTemplateRows: {
      default: "minmax(0, 1fr)",
      [breakpoints.mobile]: "auto minmax(0, 1fr)",
    },
    position: "sticky",
    height: "100svh",
    top: 0,
  },
  words: {
    minWidth: 0,
  },
  name: {
    gap: spacing["3"],
    margin: 0,
    alignItems: "baseline",
    color: colors.foreground,
    display: "flex",
    fontFamily: display.fontFamily,
    fontSize: { default: "2rem", [breakpoints.mobile]: "1.5rem" },
    fontVariationSettings: '"opsz" 48',
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: 1.1,
  },
  number: {
    color: linen.thread,
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.75rem",
    letterSpacing: "0.04em",
  },
  captions: {
    marginTop: { default: spacing["6"], [breakpoints.mobile]: spacing["3"] },
  },
  captionsLive: {
    display: "grid",
  },
  caption: {
    margin: 0,
    marginTop: spacing["4"],
    maxWidth: "30rem",
    color: colors.mutedForeground,
    fontSize: { default: "1.0625rem", [breakpoints.mobile]: "0.9375rem" },
    lineHeight: { default: "1.75rem", [breakpoints.mobile]: "1.4rem" },
    textWrap: "pretty",
  },
  captionLive: {
    marginTop: 0,
    gridArea: "1 / 1",
    opacity: "clamp(0, calc(1 - max(var(--step) - var(--k), var(--k) - var(--step))), 1)",
    transform:
      "translateY(calc(clamp(-1, calc(var(--k) - var(--step)), 1) * 0.75rem))",
    transitionDuration: "calc(var(--motion, 0) * 450ms)",
    transitionProperty: "opacity, transform",
    transitionTimingFunction: "cubic-bezier(0.22, 1, 0.36, 1)",
  },
  dots: {
    gap: spacing["1.5"],
    display: "flex",
    marginTop: { default: "2rem", [breakpoints.mobile]: spacing["3"] },
  },
  dot: {
    backgroundColor: linen.thread,
    height: 2,
    opacity: "calc(0.25 + 0.75 * clamp(0, calc(var(--step) - var(--k) + 1), 1))",
    transitionDuration: "calc(var(--motion, 0) * 300ms)",
    transitionProperty: "opacity",
    width: "1.5rem",
  },
  picture: {
    minWidth: 0,
    position: "relative",
    height: `calc(${HEIGHT}px * var(--scale, 0.75))`,
  },
  pictureLive: {
    height: "100%",
  },
  canvas: {
    position: "absolute",
    left: "50%",
    top: "50%",
    height: HEIGHT,
    transform: "translate(-50%, -50%) scale(var(--scale, 0.75))",
    width: WIDTH,
  },
});
