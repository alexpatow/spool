// The studio: what a running server holds, read through its API, and the
// changes a reviewer makes to it.
//
// Every screen is one or a few calls a client could make, shown, and every
// change is one more; the path of the address says which screen.

import * as stylex from "@stylexjs/stylex";
import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { SpoolClient, SpoolError } from "./api.ts";
import type { Fetch, Info } from "./api.ts";
import { Code, Failure, Labelled, LinkTo, Notice, Page, Quiet, Text } from "./components.tsx";
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
import { StudioContext, useRoute } from "./hooks.ts";
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
  RecordIcon,
  RefreshIcon,
  RunIcon,
  SearchIcon,
  TokenIcon,
} from "./icons.tsx";
import { Palette } from "./palette.tsx";
import { href } from "./router.ts";
import type { Route } from "./router.ts";
import { Document, Documents } from "./screens/documents.tsx";
import { Draft, Drafts } from "./screens/drafts.tsx";
import { Explain } from "./screens/explain.tsx";
import { Identities, Identity } from "./screens/identities.tsx";
import { Changeset, Log } from "./screens/log.tsx";
import { Observation, Observations } from "./screens/observations.tsx";
import { Overview } from "./screens/overview.tsx";
import { ReviewItem, ReviewItems } from "./screens/review.tsx";
import { Run, Runs } from "./screens/runs.tsx";
import { colors, radius, spacing, typography } from "./styles/shelf/tokens.stylex";
import { applyTheme } from "./styles/themes.ts";
import type { Theme } from "./styles/themes.ts";
import { tones } from "./styles/tones.stylex";

/** Where the token is kept: for as long as the tab is open, and no longer. */
export const TOKEN_KEY = "spool.token";

/** The token of `spool dev`, which is tried when none was entered. */
export const DEV_TOKEN = "dev";

/** Where the reviewer's name is kept. It is no secret, so it outlives the tab. */
export const REVIEWER_KEY = "spool.reviewer";

/** Where it is kept whether the reader chose a light or a dark page. */
export const THEME_KEY = "spool.theme";

type Kept = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export interface AppProps {
  /** Where the server is. Default: where the page came from. */
  url?: string;
  /** The `fetch` to call with. Default: the browser's. */
  fetch?: Fetch;
  /** Where the token is kept. Default: the tab's session storage. */
  storage?: Kept;
  /** Where the reviewer's name is kept. Default: the browser's local storage. */
  names?: Kept;
}

type Connection =
  | { status: "checking" }
  | { status: "ready"; info: Info }
  /** The server refused the token. */
  | { status: "refused" }
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

export function App({
  url = window.location.origin,
  fetch,
  storage = window.sessionStorage,
  names = window.localStorage,
}: AppProps) {
  const [token, setToken] = useState(() => storage.getItem(TOKEN_KEY) ?? DEV_TOKEN);
  const [reviewer, setReviewer] = useState(() => names.getItem(REVIEWER_KEY) ?? "");
  const [version, setVersion] = useState(0);
  const [connection, setConnection] = useState<Connection>({ status: "checking" });
  const client = useMemo(() => new SpoolClient({ url, token, fetch }), [url, token, fetch]);
  const [theme, toggleTheme] = useTheme(names);

  useEffect(() => {
    let current = true;
    client.info().then(
      (info) => current && setConnection({ status: "ready", info }),
      (error: unknown) => {
        if (!current) return;
        const refused = error instanceof SpoolError && error.code === "unauthorized";
        setConnection(refused ? { status: "refused" } : { status: "failed", error });
      },
    );
    return () => {
      current = false;
    };
  }, [client, version]);

  const connect = (entered: string) => {
    storage.setItem(TOKEN_KEY, entered);
    setConnection({ status: "checking" });
    setToken(entered);
    setVersion((before) => before + 1);
  };

  if (connection.status === "checking") {
    return (
      <Door url={url}>
        <Quiet>Asking the server what it holds…</Quiet>
      </Door>
    );
  }
  if (connection.status === "refused") {
    return (
      <Door url={url}>
        <TokenForm guessed={storage.getItem(TOKEN_KEY) === null} connect={connect} />
      </Door>
    );
  }
  if (connection.status === "failed") {
    return (
      <Door url={url}>
        <Failure error={connection.error} />
        <div>
          <Button variant="outline" onClick={() => setVersion((before) => before + 1)}>
            Try again
          </Button>
        </div>
      </Door>
    );
  }
  const refresh = () => setVersion((before) => before + 1);
  const remember = (name: string) => {
    names.setItem(REVIEWER_KEY, name);
    setReviewer(name);
  };
  return (
    <StudioContext.Provider value={{ client, version, refresh, reviewer, remember, info: connection.info }}>
      <Shell
        url={url}
        info={connection.info}
        refresh={refresh}
        theme={theme}
        toggleTheme={toggleTheme}
        forget={() => {
          storage.removeItem(TOKEN_KEY);
          setConnection({ status: "refused" });
        }}
      />
    </StudioContext.Provider>
  );
}

function Mark() {
  return (
    <svg viewBox="0 0 32 32" aria-hidden {...stylex.props(styles.mark)}>
      <rect width="32" height="32" rx="8" fill="currentColor" />
      <circle cx="16" cy="16" r="7.5" fill="none" strokeWidth="3" {...stylex.props(styles.markRing)} />
      <circle cx="16" cy="16" r="2" {...stylex.props(styles.markDot)} />
    </svg>
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
          <Mark />
          <h1 {...stylex.props(styles.doorTitle)}>Spool studio</h1>
        </div>
        <Quiet>
          Server <Code>{url}</Code>
        </Quiet>
        {children}
      </div>
    </main>
  );
}

function TokenForm({ guessed, connect }: { guessed: boolean; connect: (token: string) => void }) {
  return (
    <form
      {...stylex.props(styles.tokenForm)}
      onSubmit={(event) => {
        event.preventDefault();
        const entered = String(new FormData(event.currentTarget).get("token") ?? "").trim();
        if (entered !== "") connect(entered);
      }}
    >
      {guessed ? (
        <Text>This server needs its bearer token: the value of SPOOL_TOKEN it was started with.</Text>
      ) : (
        <Notice tone="bad" role="alert">
          The server refused that token.
        </Notice>
      )}
      <Labelled label="Token">
        <Input name="token" type="password" autoComplete="off" autoFocus />
      </Labelled>
      <Button type="submit">Connect</Button>
      <Quiet>The token is kept in this tab until it is closed, and sent only to this server.</Quiet>
    </form>
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
  forget,
}: {
  url: string;
  info: Info;
  refresh: () => void;
  theme: Theme;
  toggleTheme: () => void;
  forget: () => void;
}) {
  const route = useRoute();
  const [finding, setFinding] = useState(false);
  const types = info.store.entity_types;
  const groups: Array<{ label: string; entries: Entry[] }> = [
    {
      label: "Decide",
      entries: [
        { path: "", label: "Overview", icon: <LayoutDashboardIcon /> },
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
      label: "Look",
      entries: [
        { path: "identities", label: "Identities", icon: <IdentityIcon /> },
        { path: "records", label: "Records", icon: <RecordIcon /> },
        { path: "documents", label: "Documents", icon: <FileTextIcon /> },
        { path: "explain", label: "Explain", icon: <ExplainIcon /> },
      ],
    },
    {
      label: "What happened",
      entries: [
        { path: "runs", label: "Runs", icon: <RunIcon /> },
        { path: "log", label: "Log", icon: <LogIcon /> },
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
          <a href={href([])} {...stylex.props(styles.brand)}>
            <Mark />
            <Hidden>
              <span {...stylex.props(styles.brandName)}>Spool</span>
              <span {...stylex.props(styles.brandKind)}>studio</span>
            </Hidden>
          </a>
        </SidebarHeader>
        <SidebarContent>
          <nav aria-label="Screens" {...stylex.props(styles.nav)}>
            {groups.map((group) => (
              <SidebarGroup key={group.label}>
                <SidebarGroupLabel>{group.label}</SidebarGroupLabel>
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
            <Button
              variant="ghost"
              size="sm"
              onClick={forget}
              title="Forget the token and ask for it again"
              style={styles.footButton}
            >
              <TokenIcon /> Change token
            </Button>
          </Hidden>
        </SidebarFooter>
      </Sidebar>
      <SidebarInset>
        <header {...stylex.props(styles.bar)}>
          <SidebarTrigger />
          <button type="button" onClick={() => setFinding(true)} {...stylex.props(styles.find)}>
            <SearchIcon />
            <span {...stylex.props(styles.findWords)}>Find a name, an ID, or source:key</span>
            <Kbd>⌘K</Kbd>
          </button>
          <div {...stylex.props(styles.barEnd)}>
            <Button variant="ghost" size="sm" onClick={refresh} title="Read what is shown again">
              <RefreshIcon /> Refresh
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={toggleTheme}
              aria-label={theme === "dark" ? "Make the page light" : "Make the page dark"}
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
      return id === undefined ? <Observations route={route} types={types} /> : <Observation id={id} />;
    case "documents":
      return id === undefined ? <Documents route={route} /> : <Document id={id} route={route} />;
    case "review":
      return id === undefined ? <ReviewItems route={route} types={types} /> : <ReviewItem id={id} />;
    case "drafts":
      return id === undefined ? <Drafts route={route} types={types} /> : <Draft id={id} />;
    case "log":
      return id === undefined ? <Log /> : <Changeset id={id} />;
    case "explain":
      return id === undefined ? <Explain route={route} /> : <Nowhere />;
    default:
      return <Nowhere />;
  }
}

function Nowhere() {
  return (
    <Page title="No such screen">
      <Text>
        The studio has no screen at this address. <LinkTo path={[]}>Go to the overview.</LinkTo>
      </Text>
    </Page>
  );
}

const styles = stylex.create({
  mark: {
    color: colors.primary,
    flexShrink: 0,
    height: "1.5rem",
    width: "1.5rem",
  },
  markRing: {
    stroke: colors.primaryForeground,
  },
  markDot: {
    fill: colors.primaryForeground,
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
    fontSize: typography.fontSizeLg,
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.01em",
    lineHeight: typography.lineHeightLg,
  },
  tokenForm: {
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
    fontSize: typography.fontSizeBase,
    fontWeight: typography.fontWeightSemibold,
    letterSpacing: "-0.01em",
  },
  brandKind: {
    color: colors.mutedForeground,
  },
  nav: {
    display: "flex",
    flexDirection: "column",
  },
  waiting: {
    borderRadius: radius.full,
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
