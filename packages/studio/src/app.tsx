// The studio: what a running server holds, read through its API, and the
// changes a reviewer makes to it.
//
// Every screen is one or a few calls a client could make, shown, and every
// change is one more; the path of the address says which screen.

import * as stylex from "@stylexjs/stylex";
import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { Act } from "./act.tsx";
import { SpoolClient, SpoolError } from "./api.ts";
import type { Fetch, Info, OAuthRequest, Person, SessionState } from "./api.ts";
import { Code, Failure, Labelled, Notice, Page, Quiet, Row, Text } from "./components.tsx";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Kbd } from "./components/ui/kbd";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "./components/ui/sidebar";
import { StudioContext, useRoute, useStudio } from "./hooks.ts";
import {
  DarkModeIcon,
  DraftIcon,
  ExplainIcon,
  FileTextIcon,
  IdentityIcon,
  InboxIcon,
  LayoutDashboardIcon,
  LightModeIcon,
  LogIcon,
  PasswordIcon,
  PeopleIcon,
  RecordIcon,
  RefreshIcon,
  RunIcon,
  SearchIcon,
  SignOutIcon,
  TokenIcon,
} from "./icons.tsx";
import { Palette } from "./palette.tsx";
import { here, href, parseAddress } from "./router.ts";
import type { Route } from "./router.ts";
import { Document, Documents } from "./screens/documents.tsx";
import { Draft, Drafts } from "./screens/drafts.tsx";
import { Explain } from "./screens/explain.tsx";
import { Identities, Identity } from "./screens/identities.tsx";
import { Changeset, Log } from "./screens/log.tsx";
import { RecordPage, RecordsPage } from "./screens/records.tsx";
import { Overview } from "./screens/overview.tsx";
import { NewPassword, People, password, typed } from "./screens/people.tsx";
import { Tokens } from "./screens/tokens.tsx";
import { ReviewItem, ReviewItems } from "./screens/review.tsx";
import { Run, Runs } from "./screens/runs.tsx";
import { Mark, Wordmark } from "./brand.tsx";
import { colors, display, linen, radius, spacing, typography } from "./styles/shelf/tokens.stylex";
import { applyTheme } from "./styles/themes.ts";
import type { Theme } from "./styles/themes.ts";
import { tones } from "./styles/tones.stylex";

/** Where it is kept whether the reader chose a light or a dark page. */
export const THEME_KEY = "spool.theme";

type Kept = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export interface AppProps {
  /** Where the server is. Default: where the page came from. */
  url?: string;
  /** The `fetch` to call with. Default: the browser's, which sends the session cookie. */
  fetch?: Fetch;
  /** Where the reader's choices are kept. Default: the browser's local storage. */
  storage?: Kept;
  /** Leaves the studio for another site. Default: the browser goes there. */
  leave?: (to: string) => void;
}

type Connection =
  | { status: "checking" }
  | { status: "ready"; me: Person; dev: boolean; info: Info; people: Person[] }
  /** Nobody is signed in. With `setup`, no admin can sign in yet either. */
  | { status: "out"; setup: boolean }
  | { status: "failed"; error: unknown };

/** Light or dark as the reader chose, or as their system is set when they have not. */
function useTheme(kept: Kept): [Theme, () => void] {
  const [theme, setTheme] = useState<Theme>(() => {
    const chosen = kept.getItem(THEME_KEY);
    if (chosen === "light" || chosen === "dark") return chosen;
    return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  });
  useEffect(() => applyTheme(theme), [theme]);
  return [
    theme,
    () => {
      const next = theme === "dark" ? "light" : "dark";
      kept.setItem(THEME_KEY, next);
      setTheme(next);
    },
  ];
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
function withoutRefusal(address: string): string {
  const [path = "", search = ""] = address.split("?", 2);
  const query = new URLSearchParams(search);
  query.delete("google");
  const kept = query.toString();
  return kept === "" ? path : `${path}?${kept}`;
}

export function App({
  url = window.location.origin,
  fetch,
  storage = window.localStorage,
  leave = (to) => window.location.assign(to),
}: AppProps) {
  const [version, setVersion] = useState(0);
  const [connection, setConnection] = useState<Connection>({ status: "checking" });
  const [google, setGoogle] = useState(false);
  const [turnedAway] = useState(() => new URLSearchParams(window.location.search).get("google"));
  useEffect(() => {
    // It has been read. The address is the page's again, to keep or to pass on.
    if (turnedAway !== null) {
      window.history.replaceState(window.history.state, "", withoutRefusal(here()) + window.location.hash);
    }
  }, [turnedAway]);
  const client = useMemo(() => {
    const call: Fetch = fetch ?? ((input, init) => window.fetch(input, init));
    return new SpoolClient({
      url,
      fetch: async (input, init) => {
        const response = await call(input, init);
        // The session ended while the studio was open: asking who is signed
        // in shows the door.
        if (response.status === 401 && input.pathname.startsWith("/v1/")) setVersion((before) => before + 1);
        return response;
      },
    });
  }, [url, fetch]);
  const [theme, toggleTheme] = useTheme(storage);

  useEffect(() => {
    let current = true;
    const read = async (): Promise<[Connection, boolean]> => {
      const { person, setup, dev, google } = await client.session();
      if (!person) return [{ status: "out", setup }, google];
      const [info, people] = await Promise.all([client.info(), client.people()]);
      return [{ status: "ready", me: person, dev, info, people }, google];
    };
    read().then(
      ([read, google]) => {
        if (!current) return;
        setGoogle(google);
        setConnection(read);
      },
      (error: unknown) => {
        if (!current) return;
        // The session ended between one call and the next.
        const out = error instanceof SpoolError && error.code === "unauthorized";
        setConnection(out ? { status: "out", setup: false } : { status: "failed", error });
      },
    );
    return () => {
      current = false;
    };
  }, [client, version]);

  const refresh = () => setVersion((before) => before + 1);
  const people = useMemo(
    () => new Map(connection.status === "ready" ? connection.people.map((person) => [person.id, person]) : []),
    [connection],
  );

  if (connection.status === "checking") {
    return (
      <Door url={url}>
        <Quiet>Connecting…</Quiet>
      </Door>
    );
  }
  if (connection.status === "out") {
    const entered = (state: SessionState) => {
      if (!state.person) return;
      // The link that made the first account has done its work.
      if (parseAddress(here()).path[0] === "setup") window.history.replaceState(null, "", "/");
      setConnection({ status: "checking" });
      refresh();
    };
    return (
      <Door url={url}>
        {parseAddress(here()).path[0] === "setup" ? (
          <SetupForm client={client} open={connection.setup} entered={entered} />
        ) : connection.setup ? (
          <Notice>No admin can sign in yet. Open the setup link the server printed when it started.</Notice>
        ) : (
          <SignInForm client={client} entered={entered} google={google} turnedAway={turnedAway} />
        )}
      </Door>
    );
  }
  if (connection.status === "failed") {
    return (
      <Door url={url}>
        <Failure error={connection.error} />
        <div>
          <Button variant="outline" onClick={refresh}>
            Try again
          </Button>
        </div>
      </Door>
    );
  }
  if (parseAddress(here()).path[0] === "oauth") {
    return (
      <Door url={url}>
        <Approve client={client} me={connection.me} leave={leave} />
      </Door>
    );
  }
  return (
    <StudioContext.Provider
      value={{ client, version, refresh, me: connection.me, people, google, info: connection.info }}
    >
      <Shell
        url={url}
        info={connection.info}
        refresh={refresh}
        theme={theme}
        toggleTheme={toggleTheme}
        signOut={
          connection.dev
            ? null
            : () => {
                client.signOut().then(
                  () => setConnection({ status: "out", setup: false }),
                  (error: unknown) => setConnection({ status: "failed", error }),
                );
              }
        }
      />
    </StudioContext.Provider>
  );
}

/** The site's wordmark, and what this is of Spool. Where the sidebar is narrow, the mark alone. */
function Brand({ narrow = false }: { narrow?: boolean }) {
  if (narrow) return <Mark {...stylex.props(styles.mark)} aria-label="Spool studio" role="img" />;
  return (
    <span {...stylex.props(styles.brandName)}>
      <Wordmark {...stylex.props(styles.wordmark)} />
      <span {...stylex.props(styles.brandKind)}>studio</span>
    </span>
  );
}

/** What is shown before the studio can read: the server it is asking, and what is in the way. */
function Door({ url, children }: { url: string; children: ReactNode }) {
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

function SignInForm({
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
function Approve({ client, me, leave }: { client: SpoolClient; me: Person; leave: (to: string) => void }) {
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
function SetupForm({
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

function ChangePassword() {
  return (
    <Act
      label="Change password"
      variant="ghost"
      size="sm"
      icon={<PasswordIcon />}
      buttonStyle={styles.footButton}
      act={async (client, { entered }) => {
        await client.changePassword({ current: password(entered, "current"), new: password(entered) });
        return null;
      }}
      done={() => {}}
    >
      <Labelled label="Current password">
        <Input name="current" type="password" autoComplete="current-password" required />
      </Labelled>
      <NewPassword label="New password" />
    </Act>
  );
}

interface Entry {
  path: string;
  label: string;
  icon: ReactNode;
  /** How many wait for someone, shown beside the label when there are any. */
  waiting?: number;
}

function Shell({
  url,
  info,
  refresh,
  theme,
  toggleTheme,
  signOut,
}: {
  url: string;
  info: Info;
  refresh: () => void;
  theme: Theme;
  toggleTheme: () => void;
  /** Ends the session. Null on a development server, where there is none. */
  signOut: (() => void) | null;
}) {
  const { me } = useStudio();
  const route = useRoute();
  const [finding, setFinding] = useState(false);
  const types = info.store.entity_types;
  const groups: Array<{ label: string | null; entries: Entry[] }> = [
    { label: null, entries: [{ path: "", label: "Overview", icon: <LayoutDashboardIcon /> }] },
    {
      label: "Inbox",
      entries: [
        {
          path: "review",
          label: "Review",
          icon: <InboxIcon />,
          waiting: types.reduce((sum, type) => sum + type.open_review_items, 0),
        },
        {
          path: "drafts",
          label: "Drafts",
          icon: <DraftIcon />,
          waiting: types.filter((type) => type.open_draft).length,
        },
      ],
    },
    {
      label: "Data",
      entries: [
        { path: "identities", label: "Identities", icon: <IdentityIcon /> },
        { path: "records", label: "Records", icon: <RecordIcon /> },
        { path: "documents", label: "Documents", icon: <FileTextIcon /> },
        { path: "explain", label: "Explain", icon: <ExplainIcon /> },
      ],
    },
    {
      label: "History",
      entries: [
        { path: "runs", label: "Runs", icon: <RunIcon /> },
        { path: "log", label: "Log", icon: <LogIcon /> },
      ],
    },
    {
      label: "Access",
      entries: [
        { path: "people", label: "People", icon: <PeopleIcon /> },
        // Only an admin can read them.
        ...(me.admin ? [{ path: "tokens", label: "Tokens", icon: <TokenIcon /> }] : []),
      ],
    },
  ];
  const places = useMemo(
    () => groups.flatMap((group) => group.entries.map(({ path, label, icon }) => ({ path, label, icon }))),
    // The screens are the same whatever the server holds.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  const here = route.path[0] ?? "";
  // A screen is made anew for each address: what it read belongs to the
  // address it was read for.
  const address = JSON.stringify(route);
  useEffect(() => {
    window.scrollTo?.(0, 0);
  }, [address]);

  useEffect(() => {
    const pressed = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() !== "k" || !(event.metaKey || event.ctrlKey)) return;
      event.preventDefault();
      setFinding((before) => !before);
    };
    window.addEventListener("keydown", pressed);
    return () => window.removeEventListener("keydown", pressed);
  }, []);

  return (
    <SidebarProvider>
      <Sidebar collapsible="icon">
        <SidebarHeader>
          <a href={href([])} aria-label="Spool studio" {...stylex.props(styles.brand)}>
            <SidebarBrand />
          </a>
        </SidebarHeader>
        <SidebarContent>
          <nav aria-label="Screens" {...stylex.props(styles.nav)}>
            {groups.map((group) => (
              <SidebarGroup key={group.label ?? ""}>
                {group.label ? <SidebarGroupLabel>{group.label}</SidebarGroupLabel> : null}
                <SidebarMenu>
                  {group.entries.map((entry) => (
                    <SidebarMenuItem key={entry.path}>
                      <SidebarMenuButton
                        isActive={entry.path === here}
                        tooltip={entry.label}
                        render={
                          <a
                            href={href(entry.path === "" ? [] : [entry.path])}
                            aria-current={entry.path === here ? "page" : undefined}
                          />
                        }
                      >
                        {entry.icon}
                        <span>{entry.label}</span>
                        {entry.waiting ? (
                          <span data-part="waiting" {...stylex.props(styles.waiting)}>
                            {entry.waiting}
                          </span>
                        ) : null}
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  ))}
                </SidebarMenu>
              </SidebarGroup>
            ))}
          </nav>
        </SidebarContent>
        <SidebarFooter>
          <Hidden>
            <div title={url} {...stylex.props(styles.server)}>
              <span {...stylex.props(styles.serverMark)} />
              <span {...stylex.props(styles.serverName)}>{url.replace(/^https?:\/\//, "")}</span>
            </div>
            <div data-part="me" title={me.username} {...stylex.props(styles.me)}>
              {me.name}
            </div>
            {signOut ? (
              <>
                {/* Someone who signs in with Google has none to change. */}
                {me.password ? <ChangePassword /> : null}
                <Button variant="ghost" size="sm" onClick={signOut} style={styles.footButton}>
                  <SignOutIcon /> Sign out
                </Button>
              </>
            ) : null}
          </Hidden>
        </SidebarFooter>
      </Sidebar>
      <SidebarInset>
        <header {...stylex.props(styles.bar)}>
          <SidebarTrigger />
          <button type="button" onClick={() => setFinding(true)} {...stylex.props(styles.find)}>
            <SearchIcon />
            <span {...stylex.props(styles.findWords)}>Search</span>
            <Kbd>⌘K</Kbd>
          </button>
          <div {...stylex.props(styles.barEnd)}>
            <Button variant="ghost" size="sm" onClick={refresh}>
              <RefreshIcon /> Refresh
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={toggleTheme}
              aria-label={theme === "dark" ? "Light theme" : "Dark theme"}
            >
              {theme === "dark" ? <LightModeIcon /> : <DarkModeIcon />}
            </Button>
          </div>
        </header>
        <div key={address} {...stylex.props(styles.screen)}>
          <Screen route={route} types={types.map((type) => type.name)} />
        </div>
      </SidebarInset>
      <Palette open={finding} onOpenChange={setFinding} places={places} />
    </SidebarProvider>
  );
}

function SidebarBrand() {
  const { open, isMobile } = useSidebar();
  return <Brand narrow={!open && !isMobile} />;
}

/** What the sidebar shows only while it is wide enough to. */
function Hidden({ children }: { children: ReactNode }) {
  const { open, isMobile } = useSidebar();
  return open || isMobile ? <>{children}</> : null;
}

function Screen({ route, types }: { route: Route; types: string[] }) {
  const [screen, id, extra] = route.path;
  if (extra !== undefined) return <Nowhere />;
  switch (screen) {
    case undefined:
      return id === undefined ? <Overview /> : <Nowhere />;
    case "runs":
      return id === undefined ? <Runs route={route} types={types} /> : <Run id={id} />;
    case "identities":
      return id === undefined ? <Identities route={route} types={types} /> : <Identity id={id} />;
    case "records":
      return id === undefined ? <RecordsPage route={route} types={types} /> : <RecordPage id={id} />;
    case "documents":
      return id === undefined ? <Documents route={route} /> : <Document id={id} route={route} />;
    case "review":
      return id === undefined ? <ReviewItems route={route} types={types} /> : <ReviewItem id={id} />;
    case "drafts":
      return id === undefined ? <Drafts route={route} types={types} /> : <Draft id={id} />;
    case "log":
      return id === undefined ? <Log route={route} /> : <Changeset id={id} />;
    case "explain":
      return id === undefined ? <Explain route={route} /> : <Nowhere />;
    case "people":
      return id === undefined ? <People /> : <Nowhere />;
    case "tokens":
      return id === undefined ? <Tokens /> : <Nowhere />;
    default:
      return <Nowhere />;
  }
}

function Nowhere() {
  return (
    <Page title="Not found">
      <Text>No screen at this address.</Text>
    </Page>
  );
}

const styles = stylex.create({
  mark: {
    color: colors.foreground,
    flexShrink: 0,
    height: "1.5rem",
    width: "1.5rem",
  },
  wordmark: {
    color: colors.foreground,
    display: "block",
    flexShrink: 0,
  },
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
  brand: {
    gap: spacing["2"],
    padding: spacing["1"],
    alignItems: "center",
    color: colors.foreground,
    display: "flex",
    textDecorationLine: "none",
  },
  brandName: {
    gap: spacing["2"],
    alignItems: "center",
    display: "inline-flex",
  },
  // Set as the site sets its headlines, beside the wordmark.
  brandKind: {
    color: colors.mutedForeground,
    fontFamily: display.fontFamily,
    fontSize: typography.fontSizeLg,
    fontVariationSettings: '"opsz" 24',
    fontWeight: linen.displayWeight,
    letterSpacing: display.tracking,
    lineHeight: 1,
    paddingTop: "0.125rem",
  },
  nav: {
    display: "flex",
    flexDirection: "column",
  },
  waiting: {
    borderRadius: radius.sm,
    paddingInline: spacing["1.5"],
    backgroundColor: tones.waitsSurface,
    color: tones.waits,
    fontSize: typography.fontSizeXs,
    fontVariantNumeric: "tabular-nums",
    fontWeight: typography.fontWeightMedium,
    justifySelf: "end",
    lineHeight: "1.125rem",
    marginInlineStart: "auto",
    minWidth: "1.125rem",
    textAlign: "center",
    boxSizing: "border-box",
  },
  server: {
    gap: spacing["2"],
    paddingInline: spacing["2"],
    alignItems: "center",
    color: colors.mutedForeground,
    display: "flex",
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
    minWidth: 0,
  },
  serverMark: {
    borderRadius: radius.full,
    backgroundColor: tones.goodMark,
    flexShrink: 0,
    height: "0.375rem",
    width: "0.375rem",
  },
  serverName: {
    fontFamily: typography.fontFamilyMono,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  me: {
    paddingInline: spacing["2"],
    fontWeight: typography.fontWeightMedium,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  footButton: {
    color: colors.mutedForeground,
    justifyContent: "flex-start",
  },
  bar: {
    borderBottomColor: colors.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    gap: spacing["3"],
    paddingInline: spacing["4"],
    alignItems: "center",
    backgroundColor: colors.background,
    display: "flex",
    flexShrink: 0,
    position: "sticky",
    zIndex: 10,
    height: "3rem",
    top: 0,
  },
  find: {
    borderColor: colors.input,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["2"],
    paddingInline: spacing["2.5"],
    alignItems: "center",
    backgroundColor: { default: colors.background, ":hover": colors.card },
    color: colors.mutedForeground,
    cursor: "pointer",
    display: "flex",
    flexGrow: 1,
    fontFamily: typography.fontFamily,
    fontSize: typography.fontSizeSm,
    height: "2rem",
    maxWidth: "26rem",
    minWidth: 0,
    outlineColor: colors.ring,
    outlineOffset: 2,
    outlineStyle: { default: "none", ":focus-visible": "solid" },
    outlineWidth: 2,
  },
  findWords: {
    flexGrow: 1,
    overflow: "hidden",
    textAlign: "start",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  barEnd: {
    gap: spacing["1"],
    alignItems: "center",
    display: "flex",
    marginInlineStart: "auto",
  },
  screen: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: 0,
  },
});
