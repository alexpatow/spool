// What a reviewer changes from the studio. Each change is one call of the
// API, and the engine decides whether it holds: the studio sends what was
// asked for and shows what the engine answered.

import * as stylex from "@stylexjs/stylex";
import { useQueryClient } from "@tanstack/react-query";
import type { QueryKey } from "@tanstack/react-query";
import { useRef, useState } from "react";
import type { ComponentProps, ReactNode } from "react";

import { SpoolError } from "./api.ts";
import type { Committed, Op, SpoolClient } from "./api.ts";
import { Failure, Lines, Notice } from "./components/layout.tsx";
import { Ops, Parts, Ref } from "./components/refs.tsx";
import { Labelled } from "./components/forms.tsx";
import { Button } from "spool-ui/components/button";
import type { ButtonSize, ButtonVariant } from "spool-ui/components/button";
import * as Dialog from "spool-ui/components/dialog";
import { Input } from "spool-ui/components/input";
import { describeOp } from "./format.ts";
import { useStudio } from "./hooks.ts";
import { useOpNames } from "./queries.ts";
import { colors, radius, spacing, typography } from "spool-ui/tokens.stylex";

/** Something the reviewer left out, which the studio can say without asking the server. */
export class Mistake extends Error {}

/** What the reviewer entered with a change. */
export interface Said {
  /** Left out when none was entered. */
  reason?: string;
  /** Everything the form holds. */
  entered: FormData;
}

export type Change = (client: SpoolClient, said: Said) => Promise<ReactNode>;

/**
 * A change the reviewer can make: the button that proposes it, and the
 * dialog that says what it would do, takes what it needs and makes it.
 * `act` makes the call and answers with what to say once it is made. What
 * the change made `stale` is then read again, which may take the button
 * away, so the screen keeps what was said and is told through `done`.
 */
export function Act({
  label,
  title,
  confirm,
  about,
  variant = "outline",
  size = "default",
  icon,
  buttonStyle,
  why,
  look,
  children,
  act,
  stale,
  done,
}: {
  label: string;
  /** What the dialog is called. Default: the label. */
  title?: string;
  /** What the button that makes the change says. Default: the label. */
  confirm?: string;
  /** What the change would do, said before it is made. */
  about?: ReactNode;
  variant?: ButtonVariant;
  size?: ButtonSize;
  icon?: ReactNode;
  buttonStyle?: ComponentProps<typeof Button>["style"];
  /** The label of the field for a reason. Without one, no reason is asked for. */
  why?: string;
  /** A second button that changes nothing, and shows what it answers in the dialog. */
  look?: { label: string; act: Change };
  /** The fields that are the change's own. */
  children?: ReactNode;
  act: Change;
  /** The reads the change makes stale, by their keys: one of `stale` in `queries.ts`. */
  stale: readonly QueryKey[];
  done: (said: ReactNode) => void;
}) {
  const { client } = useStudio();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<unknown>(null);
  const [looked, setLooked] = useState<ReactNode>(null);

  const said = (form: HTMLFormElement): Said => {
    const entered = new FormData(form);
    const reason = String(entered.get("reason") ?? "").trim();
    return { reason: reason === "" ? undefined : reason, entered };
  };

  // A second press that arrives before the buttons are shown as disabled
  // must not make the change twice.
  const running = useRef(false);

  const start = (form: HTMLFormElement, change: Change, then: (answer: ReactNode) => void) => {
    if (running.current) return;
    setProblem(null);
    setLooked(null);
    let entered: Said;
    try {
      entered = said(form);
    } catch (error) {
      setProblem(error);
      return;
    }
    running.current = true;
    setBusy(true);
    const ended = () => {
      running.current = false;
      setBusy(false);
    };
    change(client, entered).then(
      (answer) => {
        ended();
        then(answer);
      },
      (error: unknown) => {
        ended();
        setProblem(error);
      },
    );
  };

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        // A dialog that closed with a change under way would hide what came of it.
        if (running.current) return;
        setOpen(next);
        setProblem(null);
        setLooked(null);
      }}
    >
      <Dialog.Trigger render={<Button variant={variant} size={size} style={buttonStyle} />}>
        {icon}
        {label}
      </Dialog.Trigger>
      <Dialog.Content style={styles.dialog}>
        <form
          {...stylex.props(styles.form)}
          onSubmit={(event) => {
            event.preventDefault();
            start(event.currentTarget, act, (answer) => {
              setOpen(false);
              done(answer);
              for (const queryKey of stale) void queryClient.invalidateQueries({ queryKey });
            });
          }}
        >
          <Dialog.Header>
            <Dialog.Title>{title ?? label}</Dialog.Title>
            {about ? <Dialog.Description render={<div />}>{about}</Dialog.Description> : null}
          </Dialog.Header>
          {children}
          {why ? (
            <Labelled label={why}>
              <Input name="reason" autoComplete="off" placeholder="Optional" />
            </Labelled>
          ) : null}
          {problem !== null ? <Refusal error={problem} /> : null}
          {looked !== null ? (
            <div data-part="looked" {...stylex.props(styles.looked)}>
              {looked}
            </div>
          ) : null}
          <Dialog.Footer>
            <Dialog.Close render={<Button variant="ghost" />} disabled={busy}>
              Cancel
            </Dialog.Close>
            {look ? (
              <Button
                variant="outline"
                disabled={busy}
                onClick={(event) => start(event.currentTarget.form!, look.act, setLooked)}
              >
                {look.label}
              </Button>
            ) : null}
            <Button type="submit" variant={variant === "destructive" ? "destructive" : "default"} disabled={busy}>
              {confirm ?? label}
            </Button>
          </Dialog.Footer>
        </form>
      </Dialog.Content>
    </Dialog.Root>
  );
}

/** Why a change was not made. A changeset the engine rejected is shown with the operations it names. */
function Refusal({ error }: { error: unknown }) {
  if (error instanceof Mistake) {
    return (
      <Notice tone="bad" role="alert">
        {error.message}
      </Notice>
    );
  }
  if (!(error instanceof SpoolError) || error.code !== "rejected") return <Failure error={error} />;
  const named = error.rejections.filter((rejection) => rejection.op);
  return (
    <Notice tone="bad" role="alert">
      <span>{error.message}</span>
      <span>Nothing was changed.</span>
      {named.length > 0 ? (
        <Lines>
          {named.map((rejection, index) => (
            <li key={index}>
              Operation {rejection.op_index}: <Parts parts={describeOp(rejection.op!)} />
            </li>
          ))}
        </Lines>
      ) : null}
    </Notice>
  );
}

/** What the last change on a screen came to. */
export function Done({ children }: { children: ReactNode }) {
  if (children === null || children === undefined) return null;
  return (
    <Notice tone="good" role="status">
      {children}
    </Notice>
  );
}

/** Where a committed changeset is in the log. */
export function At({ committed }: { committed: Committed }) {
  return <Ref kind="log" id={committed.seq} label={`log position ${committed.seq}`} />;
}

/** The operations of a changeset that was not committed, with records by their names. */
export function Would({ ops }: { ops: Op[] }) {
  const names = useOpNames(ops);
  return <Ops ops={ops} names={names} />;
}

const styles = stylex.create({
  dialog: {
    maxWidth: "34rem",
  },
  form: {
    gap: spacing["4"],
    display: "flex",
    flexDirection: "column",
    fontSize: typography.fontSizeSm,
    lineHeight: typography.lineHeightSm,
    minWidth: 0,
  },
  looked: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["2"],
    padding: spacing["3"],
    backgroundColor: colors.card,
    display: "flex",
    flexDirection: "column",
    maxHeight: "16rem",
    overflowY: "auto",
  },
});
