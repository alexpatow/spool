// What a reviewer changes from the studio. Each change is one call of the
// API, and the engine decides whether it holds: the studio sends what was
// asked for and shows what the engine answered.

import { useRef, useState } from "react";
import type { ReactNode } from "react";

import { SpoolError } from "./api.ts";
import type { Committed, Op, SpoolClient } from "./api.ts";
import { Failure, Ops, Parts, Ref } from "./components.tsx";
import { describeOp, observationsOf } from "./format.ts";
import { useNames, useStudio } from "./hooks.ts";

/** Something the reviewer left out, which the studio can say without asking the server. */
export class Mistake extends Error {}

/** What the reviewer entered with a change. */
export interface Said {
  /** `user:<name>`. Empty when the change does not ask who makes it. */
  actor: string;
  /** Left out when none was entered. */
  reason?: string;
  /** Everything the form holds. */
  entered: FormData;
}

export type Change = (client: SpoolClient, said: Said) => Promise<ReactNode>;

/**
 * A change the reviewer can make: its fields and the button that makes it.
 * `act` makes the call and answers with what to say once it is made. What
 * is shown is then read again, which may take this form away, so the screen
 * keeps what was said and is told through `done`.
 */
export function Act({
  label,
  who = false,
  why,
  look,
  children,
  act,
  done,
}: {
  label: string;
  /** Whether the change is recorded under the reviewer's name. */
  who?: boolean;
  /** The label of the field for a reason. Without one, no reason is asked for. */
  why?: string;
  /** A second button that changes nothing, and shows what it answers under the form. */
  look?: { label: string; act: Change };
  /** The fields that are the change's own. */
  children?: ReactNode;
  act: Change;
  done: (said: ReactNode) => void;
}) {
  const { client, refresh, reviewer, remember } = useStudio();
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<unknown>(null);
  const [looked, setLooked] = useState<ReactNode>(null);

  const said = (form: HTMLFormElement): Said => {
    const entered = new FormData(form);
    const name = String(entered.get("reviewer") ?? "").trim();
    if (who) {
      if (name === "") throw new Mistake("Say who you are: the change is recorded under your name.");
      remember(name);
    }
    const reason = String(entered.get("reason") ?? "").trim();
    return { actor: who ? `user:${name}` : "", reason: reason === "" ? undefined : reason, entered };
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
    <form
      className="filters act"
      onSubmit={(event) => {
        event.preventDefault();
        start(event.currentTarget, act, (answer) => {
          done(answer);
          refresh();
        });
      }}
    >
      {children}
      {who ? (
        <label title="The change is recorded as made by user: and this name">
          Your name
          <input
            key={reviewer}
            name="reviewer"
            defaultValue={reviewer}
            spellCheck={false}
            autoComplete="off"
            size={14}
          />
        </label>
      ) : null}
      {why ? (
        <label>
          {why}
          <input name="reason" autoComplete="off" size={32} />
        </label>
      ) : null}
      {look ? (
        <button
          type="button"
          disabled={busy}
          onClick={(event) => start(event.currentTarget.form!, look.act, setLooked)}
        >
          {look.label}
        </button>
      ) : null}
      <button type="submit" disabled={busy}>
        {label}
      </button>
      {problem !== null ? <Refusal error={problem} /> : null}
      {looked !== null ? <div className="looked">{looked}</div> : null}
    </form>
  );
}

/** Why a change was not made. A changeset the engine rejected is shown with the operations it names. */
export function Refusal({ error }: { error: unknown }) {
  if (error instanceof Mistake) {
    return (
      <p className="notice failure" role="alert">
        {error.message}
      </p>
    );
  }
  if (!(error instanceof SpoolError) || error.code !== "rejected") return <Failure error={error} />;
  const named = error.rejections.filter((rejection) => rejection.op);
  return (
    <div className="notice failure" role="alert">
      <p>{error.message}</p>
      <p>Nothing was changed.</p>
      {named.length > 0 ? (
        <ul className="plain">
          {named.map((rejection, index) => (
            <li key={index}>
              Operation {rejection.op_index}: <Parts parts={describeOp(rejection.op!)} />
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

/** What the last change on a screen came to. */
export function Done({ children }: { children: ReactNode }) {
  if (children === null || children === undefined) return null;
  return (
    <p className="notice done" role="status">
      {children}
    </p>
  );
}

/** Where a committed changeset is in the log. */
export function At({ committed }: { committed: Committed }) {
  return <Ref kind="log" id={committed.seq} label={`log position ${committed.seq}`} />;
}

/** The operations of a changeset that was not committed, with observations by their names. */
export function Would({ ops }: { ops: Op[] }) {
  const names = useNames(observationsOf(ops));
  return <Ops ops={ops} names={names} />;
}
