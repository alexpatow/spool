// The studio: what a running server holds, read through its API.
//
// It changes nothing. Every screen is one or a few calls a client could
// make, shown; the address after `#` says which screen.

import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { SpoolClient, SpoolError } from "./api.ts";
import type { Fetch, Info } from "./api.ts";
import { Failure, Page, message } from "./components.tsx";
import { findScreen } from "./find.ts";
import { StudioContext, useRoute, useStudio } from "./hooks.ts";
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

/** Where the token is kept: for as long as the tab is open, and no longer. */
export const TOKEN_KEY = "spool.token";

/** The token of `spool dev`, which is tried when none was entered. */
export const DEV_TOKEN = "dev";

export interface AppProps {
  /** Where the server is. Default: where the page came from. */
  url?: string;
  /** The `fetch` to call with. Default: the browser's. */
  fetch?: Fetch;
  /** Where the token is kept. Default: the tab's session storage. */
  storage?: Pick<Storage, "getItem" | "setItem" | "removeItem">;
}

type Connection =
  | { status: "checking" }
  | { status: "ready"; info: Info }
  /** The server refused the token. */
  | { status: "refused" }
  | { status: "failed"; error: unknown };

export function App({ url = window.location.origin, fetch, storage = window.sessionStorage }: AppProps) {
  const [token, setToken] = useState(() => storage.getItem(TOKEN_KEY) ?? DEV_TOKEN);
  const [version, setVersion] = useState(0);
  const [connection, setConnection] = useState<Connection>({ status: "checking" });
  const client = useMemo(() => new SpoolClient({ url, token, fetch }), [url, token, fetch]);

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
        <p className="quiet">Asking the server what it holds…</p>
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
        <p>
          <button type="button" onClick={() => setVersion((before) => before + 1)}>
            Try again
          </button>
        </p>
      </Door>
    );
  }
  return (
    <StudioContext.Provider value={{ client, version }}>
      <Shell
        url={url}
        info={connection.info}
        refresh={() => setVersion((before) => before + 1)}
        forget={() => {
          storage.removeItem(TOKEN_KEY);
          setConnection({ status: "refused" });
        }}
      />
    </StudioContext.Provider>
  );
}

/** What is shown before the studio can read: the server it is asking, and what is in the way. */
function Door({ url, children }: { url: string; children: ReactNode }) {
  useEffect(() => {
    document.title = "Spool studio";
  }, []);
  return (
    <main className="door">
      <h1>Spool studio</h1>
      <p className="quiet">
        Server <code>{url}</code>
      </p>
      {children}
    </main>
  );
}

function TokenForm({ guessed, connect }: { guessed: boolean; connect: (token: string) => void }) {
  return (
    <form
      className="token"
      onSubmit={(event) => {
        event.preventDefault();
        const entered = String(new FormData(event.currentTarget).get("token") ?? "").trim();
        if (entered !== "") connect(entered);
      }}
    >
      <p>
        {guessed
          ? "This server needs its bearer token: the value of SPOOL_TOKEN it was started with."
          : "The server refused that token."}
      </p>
      <label>
        Token
        <input name="token" type="password" autoComplete="off" autoFocus />
      </label>
      <button type="submit">Connect</button>
      <p className="quiet">The token is kept in this tab until it is closed, and sent only to this server.</p>
    </form>
  );
}

interface Entry {
  path: string;
  label: string;
  /** How many wait for someone, shown beside the label when there are any. */
  waiting?: number;
}

function Shell({ url, info, refresh, forget }: { url: string; info: Info; refresh: () => void; forget: () => void }) {
  const route = useRoute();
  const types = info.store.entity_types;
  const entries: Entry[] = [
    { path: "", label: "Overview" },
    { path: "runs", label: "Runs" },
    { path: "identities", label: "Identities" },
    { path: "observations", label: "Observations" },
    { path: "documents", label: "Documents" },
    { path: "review", label: "Review", waiting: types.reduce((sum, type) => sum + type.open_review_items, 0) },
    { path: "drafts", label: "Drafts", waiting: types.filter((type) => type.open_draft).length },
    { path: "log", label: "Log" },
    { path: "explain", label: "Explain" },
  ];
  const here = route.path[0] ?? "";
  // A screen is made anew for each address: what it read belongs to the
  // address it was read for.
  const address = JSON.stringify(route);
  useEffect(() => {
    window.scrollTo?.(0, 0);
  }, [address]);

  return (
    <div className="shell">
      <nav className="nav" aria-label="Screens">
        <a className="brand" href={href([])}>
          Spool <span>studio</span>
        </a>
        <ul>
          {entries.map((entry) => (
            <li key={entry.path}>
              <a
                href={href(entry.path === "" ? [] : [entry.path])}
                aria-current={entry.path === here ? "page" : undefined}
              >
                {entry.label}
                {entry.waiting ? <span className="waiting">{entry.waiting}</span> : null}
              </a>
            </li>
          ))}
        </ul>
        <p className="server" title={url}>
          <code>{url.replace(/^https?:\/\//, "")}</code>
        </p>
      </nav>
      <div className="main">
        <header className="bar">
          <Find key={address} />
          <button type="button" onClick={refresh} title="Read what is shown again">
            Refresh
          </button>
          <button type="button" className="quiet-button" onClick={forget} title="Forget the token and ask for it again">
            Change token
          </button>
        </header>
        <main key={address}>
          <Screen route={route} types={types.map((type) => type.name)} />
        </main>
      </div>
    </div>
  );
}

/** Goes to the screen of whatever a pasted ID, or a source and key, names. */
function Find() {
  const { client } = useStudio();
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <form
      className="find"
      role="search"
      onSubmit={(event) => {
        event.preventDefault();
        const form = event.currentTarget;
        const reference = String(new FormData(form).get("reference") ?? "").trim();
        if (reference === "" || busy) return;
        setBusy(true);
        setProblem(null);
        findScreen(client, reference).then(
          (screen) => {
            setBusy(false);
            if (screen === null) setProblem(`Nothing here is named ${reference}.`);
            else {
              form.reset();
              window.location.hash = screen;
            }
          },
          (error: unknown) => {
            setBusy(false);
            setProblem(message(error));
          },
        );
      }}
    >
      <input
        name="reference"
        type="search"
        aria-label="Find by ID, or by source and key"
        placeholder="Find an ID, or source:key"
        spellCheck={false}
        autoComplete="off"
        onChange={() => setProblem(null)}
      />
      <button type="submit" disabled={busy}>
        Find
      </button>
      {problem ? (
        <span className="failure" role="alert">
          {problem}
        </span>
      ) : null}
    </form>
  );
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
    case "observations":
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
      <p>
        The studio has no screen at this address. <a href={href([])}>Go to the overview.</a>
      </p>
    </Page>
  );
}
