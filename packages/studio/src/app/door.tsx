// The door: what is shown before the studio can read. Signing in, making
// the first account, and letting an assistant in as whoever is signed in.
// The address is the browser's own here: no screen is shown yet.

import * as stylex from "@stylexjs/stylex";
import { useEffect, useState } from "react";
import type { ReactNode } from "react";

import type { OAuthRequest, Person, SessionState, SpoolClient } from "../api.ts";
import { Code, Failure, Notice, Quiet, Row, Text } from "../components/layout.tsx";
import { Labelled } from "../components/forms.tsx";
import { Button } from "spool-ui/components/button";
import { Input } from "spool-ui/components/input";
import { NewPassword, password, typed } from "../screens/people.tsx";
import { colors, radius, spacing, typography } from "spool-ui/tokens.stylex";
import { Brand } from "./brand.tsx";

/** The address the browser is at: its path and its query. */
export function here(): string {
  return window.location.pathname + window.location.search;
}

/**
 * Whether the path of the address begins with a name: `setup` for the link
 * that makes the first account, `oauth` for what an assistant asks.
 */
export function isAt(name: string): boolean {
  return window.location.pathname.split("/").find((part) => part !== "") === name;
}

/** Why Google let nobody in, as the server names it in the address it sends the browser back to. */
const TURNED_AWAY: Record<string, string> = {
  unknown:
    "No account here has that Google account's email. An admin adds people, by the email they sign in to Google with.",
  unverified: "Google has not verified that account's email address, so it cannot be used to sign in here.",
  unvouched:
    "That Google account's email address is not Google's to answer for, so it cannot be used to sign in here. Use a Gmail account or one of your organisation's Google Workspace, or sign in with a password.",
  cancelled: "Sign-in with Google was cancelled.",
  expired: "That sign-in took too long, or began in another browser. Start again.",
  unreachable: "Google could not be reached. Try again in a moment.",
  busy: "Too many people are signing in at once. Try again in a moment.",
};
const NOT_TAKEN =
  "Google's answer could not be used to sign in. Try again. If it happens again, whoever runs the server can read why in its output.";

/** The address without what the server added to it to say why Google let nobody in. */
export function withoutRefusal(address: string): string {
  const [path = "", search = ""] = address.split("?", 2);
  const query = new URLSearchParams(search);
  query.delete("google");
  const kept = query.toString();
  return kept === "" ? path : `${path}?${kept}`;
}

/** What is shown before the studio can read: the server it is asking, and what is in the way. */
export function Door({ url, children }: { url: string; children: ReactNode }) {
  useEffect(() => {
    document.title = "Spool studio";
  }, []);
  return (
    <main {...stylex.props(styles.door)}>
      <div {...stylex.props(styles.doorCard)}>
        <div {...stylex.props(styles.doorHead)}>
          <h1 {...stylex.props(styles.doorTitle)}>
            <Brand />
          </h1>
        </div>
        <Quiet>
          Server <Code>{url}</Code>
        </Quiet>
        {children}
      </div>
    </main>
  );
}

/** A form of the door: what it sends, and what the server said when it would not have it. */
function DoorForm({
  label,
  send,
  entered,
  children,
}: {
  label: string;
  send: (entered: FormData) => Promise<SessionState>;
  entered: (state: SessionState) => void;
  children: ReactNode;
}) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<unknown>(null);
  return (
    <form
      {...stylex.props(styles.doorForm)}
      onSubmit={(event) => {
        event.preventDefault();
        if (busy) return;
        setBusy(true);
        setProblem(null);
        send(new FormData(event.currentTarget)).then(entered, (error: unknown) => {
          setBusy(false);
          setProblem(error);
        });
      }}
    >
      {children}
      {problem !== null ? <Failure error={problem} /> : null}
      <Button type="submit" disabled={busy}>
        {label}
      </Button>
    </form>
  );
}

function Username() {
  return (
    <Labelled label="Username">
      <Input name="username" autoComplete="username" autoCapitalize="none" spellCheck={false} required autoFocus />
    </Labelled>
  );
}

export function SignInForm({
  client,
  entered,
  google,
  turnedAway,
}: {
  client: SpoolClient;
  entered: (state: SessionState) => void;
  /** Whether the server signs people in with Google. */
  google: boolean;
  /** Why Google let nobody in, if the browser has just come back from it. */
  turnedAway: string | null;
}) {
  return (
    <>
      {turnedAway !== null ? (
        <Notice tone="bad" role="alert">
          {TURNED_AWAY[turnedAway] ?? NOT_TAKEN}
        </Notice>
      ) : null}
      {google ? (
        <>
          {/* Google sends the browser back to the page it is on. */}
          <Button
            variant="outline"
            nativeButton={false}
            render={<a href={client.googleSignIn(withoutRefusal(here()))} />}
          >
            Sign in with Google
          </Button>
          <Quiet>Or with a password:</Quiet>
        </>
      ) : null}
      <DoorForm
        label="Sign in"
        send={(form) => client.signIn(typed(form, "username"), password(form))}
        entered={entered}
      >
        <Username />
        <Labelled label="Password">
          <Input name="password" type="password" autoComplete="current-password" required />
        </Labelled>
      </DoorForm>
    </>
  );
}

/**
 * An assistant such as Claude or ChatGPT on the web asks to work as whoever
 * is signed in. The address is the one it sent the browser to, and its query
 * is what it asks for, which the server checks again when it is answered.
 */
export function Approve({ client, me, leave }: { client: SpoolClient; me: Person; leave: (to: string) => void }) {
  const request = window.location.search.slice(1);
  const [asked, setAsked] = useState<OAuthRequest | null>(null);
  const [problem, setProblem] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let current = true;
    client.oauthRequest(request).then(
      (read) => current && setAsked(read),
      (error: unknown) => current && setProblem(error),
    );
    return () => {
      current = false;
    };
  }, [client, request]);
  const decide = (approve: boolean) => {
    setBusy(true);
    setProblem(null);
    client.decideOAuth(request, approve).then(
      ({ to }) => leave(to),
      (error: unknown) => {
        setBusy(false);
        setProblem(error);
      },
    );
  };
  if (asked === null) {
    return problem === null ? (
      <Quiet>Reading what the assistant asks for…</Quiet>
    ) : (
      <>
        <Failure error={problem} />
        <Quiet>Go back to the assistant and connect it to this server again.</Quiet>
      </>
    );
  }
  return (
    <>
      <Text>
        <strong>{asked.client_name ?? "An assistant"}</strong> asks to read and change what this server holds, as you,{" "}
        {me.name}. What it changes is recorded under your name.
      </Text>
      <Text>
        Approving sends you to <strong data-part="host">{asked.host}</strong>.
      </Text>
      {asked.loopback ? (
        <Notice tone="waits">
          That is a program on this computer, and any program here could be it. Approve only if you have just connected
          an assistant on this computer, such as Claude Code.
        </Notice>
      ) : null}
      {problem !== null ? <Failure error={problem} /> : null}
      <Row>
        <Button disabled={busy} onClick={() => decide(true)}>
          Approve
        </Button>
        <Button variant="outline" disabled={busy} onClick={() => decide(false)}>
          Turn away
        </Button>
      </Row>
    </>
  );
}

/**
 * Makes the first account. The address carries the code the server printed,
 * after the `#`, where it is sent to no server and kept in no log.
 */
export function SetupForm({
  client,
  open,
  entered,
}: {
  client: SpoolClient;
  /** Whether the server still waits for its first account. */
  open: boolean;
  entered: (state: SessionState) => void;
}) {
  const code = window.location.hash.slice(1);
  if (!open) {
    return (
      <>
        <Notice>This server is already set up.</Notice>
        <div>
          <Button variant="outline" render={<a href="/" />}>
            Sign in
          </Button>
        </div>
      </>
    );
  }
  return (
    <DoorForm
      label="Create admin account"
      send={(form) =>
        client.setUp({
          code: code === "" ? typed(form, "code") : code,
          username: typed(form, "username"),
          name: typed(form, "name"),
          password: password(form),
        })
      }
      entered={entered}
    >
      <Text>The first account. It is an admin, and adds everyone else.</Text>
      {code === "" ? (
        <Labelled label="Setup code" hint="The part after # in the link the server printed.">
          <Input name="code" autoComplete="off" spellCheck={false} required />
        </Labelled>
      ) : null}
      <Labelled label="Name">
        <Input name="name" autoComplete="name" required />
      </Labelled>
      <Username />
      <NewPassword />
    </DoorForm>
  );
}

const styles = stylex.create({
  door: {
    padding: spacing["6"],
    alignItems: "center",
    backgroundColor: colors.card,
    boxSizing: "border-box",
    color: colors.foreground,
    display: "flex",
    fontFamily: typography.fontFamily,
    fontSize: typography.fontSizeSm,
    justifyContent: "center",
    lineHeight: typography.lineHeightSm,
    minHeight: "100svh",
  },
  doorCard: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["4"],
    padding: spacing["6"],
    backgroundColor: colors.background,
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    maxWidth: "26rem",
    width: "100%",
  },
  doorHead: {
    gap: spacing["2.5"],
    alignItems: "center",
    display: "flex",
  },
  doorTitle: {
    margin: 0,
    display: "flex",
  },
  doorForm: {
    gap: spacing["4"],
    display: "flex",
    flexDirection: "column",
  },
});
