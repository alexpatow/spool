// The shell: what is around every screen once the studio can read. The
// list of the screens, who is signed in, and the bar that finds things.

import * as stylex from "@stylexjs/stylex";
import { useQueryClient } from "@tanstack/react-query";
import { Outlet, useLocation, useMatches } from "@tanstack/react-router";
import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { Act } from "../act.tsx";
import { Page, Text } from "../components/layout.tsx";
import { ScreenLink } from "../components/refs.tsx";
import { Labelled } from "../components/forms.tsx";
import { Button } from "spool-ui/components/button";
import { Input } from "spool-ui/components/input";
import { Kbd } from "spool-ui/components/kbd";
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
} from "spool-ui/components/sidebar";
import { useStudio } from "../hooks.ts";
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
} from "../icons.tsx";
import { Palette } from "../palette.tsx";
import { stale } from "../queries.ts";
import type { Place } from "../palette.tsx";
import { NewPassword, password } from "../screens/people.tsx";
import { colors, radius, spacing, typography } from "spool-ui/tokens.stylex";
import type { Theme } from "../styles/themes.ts";
import { tones } from "../styles/tones.stylex";
import { Brand } from "./brand.tsx";

/** What the shell shows that is the page's and not the store's. */
export interface Around {
  /** Where the server is. */
  url: string;
  theme: Theme;
  toggleTheme(): void;
  /** Ends the session. Null on a development server, where there is none. */
  signOut: (() => void) | null;
}

export const AroundContext = createContext<Around | null>(null);

function ChangePassword() {
  return (
    <Act
      stale={stale.people}
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

interface Entry extends Place {
  /** How many wait for someone, shown beside the label when there are any. */
  waiting?: number;
}

/** The studio around the screen the address names, which the router puts in its place. */
export function Shell() {
  const around = useContext(AroundContext);
  if (around === null) throw new Error("the shell is not in the studio");
  const { url, theme, toggleTheme, signOut } = around;
  const { me, info } = useStudio();
  const queryClient = useQueryClient();
  const [finding, setFinding] = useState(false);
  const types = info.store.entity_types;
  const groups: Array<{ label: string | null; entries: Entry[] }> = [
    { label: null, entries: [{ to: "/", label: "Overview", icon: <LayoutDashboardIcon /> }] },
    {
      label: "Inbox",
      entries: [
        {
          to: "/review",
          label: "Review",
          icon: <InboxIcon />,
          waiting: types.reduce((sum, type) => sum + type.open_review_items, 0),
        },
        {
          to: "/drafts",
          label: "Drafts",
          icon: <DraftIcon />,
          waiting: types.filter((type) => type.open_draft).length,
        },
      ],
    },
    {
      label: "Data",
      entries: [
        { to: "/identities", label: "Identities", icon: <IdentityIcon /> },
        { to: "/records", label: "Records", icon: <RecordIcon /> },
        { to: "/documents", label: "Documents", icon: <FileTextIcon /> },
        { to: "/explain", label: "Explain", icon: <ExplainIcon /> },
      ],
    },
    {
      label: "History",
      entries: [
        { to: "/runs", label: "Runs", icon: <RunIcon /> },
        { to: "/log", label: "Log", icon: <LogIcon /> },
      ],
    },
    {
      label: "Access",
      entries: [
        { to: "/people", label: "People", icon: <PeopleIcon /> },
        // Only an admin can read them.
        ...(me.admin ? [{ to: "/tokens" as const, label: "Tokens", icon: <TokenIcon /> }] : []),
      ],
    },
  ];
  const places = useMemo(
    () => groups.flatMap((group) => group.entries.map(({ to, label, icon }): Place => ({ to, label, icon }))),
    // The screens are the same whatever the server holds.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  // The screen an address is of is named by the first part of its path.
  const here = useLocation({ select: (location) => `/${location.pathname.split("/")[1] ?? ""}` });
  // A screen is made anew for each address: what it read belongs to the
  // address it was read for. The address is that of the screen the router
  // shows, which changes in the same moment as the screen does.
  const address = useMatches({
    select: (matches) => {
      const shown = matches.at(-1)!;
      return JSON.stringify([shown.pathname, shown.search]);
    },
  });
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
          <ScreenLink to="/" aria-label="Spool studio" {...stylex.props(styles.brand)}>
            <SidebarBrand />
          </ScreenLink>
        </SidebarHeader>
        <SidebarContent>
          <nav aria-label="Screens" {...stylex.props(styles.nav)}>
            {groups.map((group) => (
              <SidebarGroup key={group.label ?? ""}>
                {group.label ? <SidebarGroupLabel>{group.label}</SidebarGroupLabel> : null}
                <SidebarMenu>
                  {group.entries.map((entry) => (
                    <SidebarMenuItem key={entry.to}>
                      <SidebarMenuButton
                        isActive={entry.to === here}
                        tooltip={entry.label}
                        render={<ScreenLink to={entry.to} current={entry.to === here ? "page" : undefined} />}
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
            <Button variant="ghost" size="sm" onClick={() => void queryClient.invalidateQueries()}>
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
          <Outlet />
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

/** What is shown at an address no screen has. */
export function Nowhere() {
  return (
    <Page title="Not found">
      <Text>No screen at this address.</Text>
    </Page>
  );
}

const styles = stylex.create({
  brand: {
    gap: spacing["2"],
    padding: spacing["1"],
    alignItems: "center",
    color: colors.foreground,
    display: "flex",
    textDecorationLine: "none",
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
