// The studio: what a running server holds, read through its API, and the
// changes a reviewer makes to it.
//
// Every screen is one or a few calls a client could make, shown, and every
// change is one more; the path of the address says which screen. The door
// (`app/door.tsx`) is shown until someone is signed in, and the shell
// (`app/shell.tsx`) around every screen after.

import { createBrowserHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";

import { parseSearch, stringifySearch } from "./address.ts";
import { SpoolClient, SpoolError } from "./api.ts";
import type { Fetch, Info, Person, SessionState } from "./api.ts";
import { Approve, Door, SetupForm, SignInForm, here, isAt, withoutRefusal } from "./app/door.tsx";
import { AroundContext } from "./app/shell.tsx";
import { Failure, Notice, Quiet } from "./components.tsx";
import { Button } from "spool-ui/components/button";
import { StudioContext } from "./hooks.ts";
import { routeTree } from "./routeTree.gen.ts";
import { applyTheme } from "./styles/themes.ts";
import type { Theme } from "./styles/themes.ts";

/** Where it is kept whether the reader chose a light or a dark page. */
export const THEME_KEY = "spool.theme";

type Kept = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/**
 * The address of the page, as the router reads and writes it. A page has
 * one, whatever is shown in it. The router writes an address a moment after
 * it goes there, and `flush` writes it at once.
 */
export const addresses = createBrowserHistory();

/**
 * What shows the screen an address names. The query of an address is text,
 * written as `address.ts` says, and each route reads its own filters out of
 * it.
 */
export function createStudioRouter() {
  return createRouter({ routeTree, history: addresses, parseSearch, stringifySearch });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createStudioRouter>;
  }
}

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

export function App({
  url = window.location.origin,
  fetch,
  storage = window.localStorage,
  leave = (to) => window.location.assign(to),
}: AppProps) {
  const [version, setVersion] = useState(0);
  const [connection, setConnection] = useState<Connection>({ status: "checking" });
  const [google, setGoogle] = useState(false);
  const [router] = useState(createStudioRouter);
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
      if (isAt("setup")) window.history.replaceState(null, "", "/");
      setConnection({ status: "checking" });
      refresh();
    };
    return (
      <Door url={url}>
        {isAt("setup") ? (
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
  if (isAt("oauth")) {
    return (
      <Door url={url}>
        <Approve client={client} me={connection.me} leave={leave} />
      </Door>
    );
  }
  const signOut = connection.dev
    ? null
    : () => {
        client.signOut().then(
          () => setConnection({ status: "out", setup: false }),
          (error: unknown) => setConnection({ status: "failed", error }),
        );
      };
  return (
    <StudioContext.Provider
      value={{ client, version, refresh, me: connection.me, people, google, info: connection.info }}
    >
      <AroundContext.Provider value={{ url, theme, toggleTheme, signOut }}>
        <RouterProvider router={router} />
      </AroundContext.Provider>
    </StudioContext.Provider>
  );
}
