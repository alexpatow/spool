// What every screen has: the client, a way to read with it, and the address.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";

import type { SpoolClient } from "./api.ts";
import { found } from "./find.ts";
import { observationName } from "./format.ts";
import { parseHash } from "./router.ts";
import type { Route } from "./router.ts";

export interface Studio {
  client: SpoolClient;
  /** Counts the times what is shown was read again. */
  version: number;
  /** Reads what is shown again, as after a change. */
  refresh(): void;
  /** The name the reviewer last made a change under. Empty before the first. */
  reviewer: string;
  remember(reviewer: string): void;
}

export const StudioContext = createContext<Studio | null>(null);

export function useStudio(): Studio {
  const studio = useContext(StudioContext);
  if (studio === null) throw new Error("the studio is not connected");
  return studio;
}

export function useRoute(): Route {
  const [route, setRoute] = useState(() => parseHash(window.location.hash));
  useEffect(() => {
    const changed = () => setRoute(parseHash(window.location.hash));
    window.addEventListener("hashchange", changed);
    return () => window.removeEventListener("hashchange", changed);
  }, []);
  return route;
}

export type Loaded<T> = { status: "loading" } | { status: "failed"; error: unknown } | { status: "ready"; data: T };

/**
 * Reads when the screen appears, and again when the reader asks to. What
 * was read stays on the screen until the new answer is there.
 */
export function useLoad<T>(load: (client: SpoolClient) => Promise<T>): Loaded<T> {
  const { client, version } = useStudio();
  const [state, setState] = useState<Loaded<T>>({ status: "loading" });
  // A screen is made anew for each address, so what it reads does not
  // change while it is there: the latest `load` is as good as the first.
  const latest = useRef(load);
  latest.current = load;
  useEffect(() => {
    let current = true;
    latest.current(client).then(
      (data) => current && setState({ status: "ready", data }),
      (error: unknown) => current && setState({ status: "failed", error }),
    );
    return () => {
      current = false;
    };
  }, [client, version]);
  return state;
}

export interface Page<T> {
  items: T[];
  /** What to pass for the page after. Nothing on the last page. */
  next?: string | number | null;
}

export interface Pages<T> {
  loaded: Loaded<T[]>;
  /** Whether there is a page after those read. */
  more: boolean;
  reading: boolean;
  readMore(): void;
}

/** A list that is read a page at a time, as far as the reader asks. */
export function usePages<T>(
  read: (client: SpoolClient, after: string | number | undefined) => Promise<Page<T>>,
): Pages<T> {
  const { client, version } = useStudio();
  const [loaded, setLoaded] = useState<Loaded<T[]>>({ status: "loading" });
  const [next, setNext] = useState<string | number | null>(null);
  const [reading, setReading] = useState(false);
  const latest = useRef(read);
  latest.current = read;
  // Which reading of the list is the current one: a page that arrives after
  // the reader asked to read again belongs to a list that is gone.
  const generation = useRef(0);

  useEffect(() => {
    const mine = ++generation.current;
    setReading(false);
    latest.current(client, undefined).then(
      (page) => {
        if (generation.current !== mine) return;
        setLoaded({ status: "ready", data: page.items });
        setNext(page.next ?? null);
      },
      (error: unknown) => generation.current === mine && setLoaded({ status: "failed", error }),
    );
    return () => {
      generation.current++;
    };
  }, [client, version]);

  const readMore = useCallback(() => {
    if (next === null || reading) return;
    const mine = generation.current;
    setReading(true);
    latest.current(client, next).then(
      (page) => {
        if (generation.current !== mine) return;
        setLoaded((before) => ({
          status: "ready",
          data: [...(before.status === "ready" ? before.data : []), ...page.items],
        }));
        setNext(page.next ?? null);
        setReading(false);
      },
      (error: unknown) => {
        if (generation.current !== mine) return;
        setLoaded({ status: "failed", error });
        setReading(false);
      },
    );
  }, [client, next, reading]);

  return { loaded, more: next !== null, reading, readMore };
}

/** The most observations one screen looks up to show their names in place of their IDs. */
const MOST_NAMES = 100;

/**
 * What to call the observations a screen names only by ID, such as those in
 * the operations of a changeset: the source and key of each. `known` are
 * those the screen has read already. An observation that is not looked up,
 * or not found, is shown by its ID.
 */
export function useNames(ids: string[], known?: Map<string, string>): Map<string, string> {
  const { client } = useStudio();
  const [looked, setLooked] = useState<Map<string, string>>(new Map());
  const wanted = [...new Set(ids)].filter((id) => !known?.has(id)).slice(0, MOST_NAMES);
  const key = wanted.join();
  useEffect(() => {
    let current = true;
    const ids = key === "" ? [] : key.split(",");
    // A name is a convenience: if one cannot be read, the ID stays.
    Promise.all(ids.map((id) => found(client.observation(id)).catch(() => null))).then((records) => {
      if (!current) return;
      setLooked(new Map(records.flatMap((record) => (record ? [[record.id, observationName(record)]] : []))));
    });
    return () => {
      current = false;
    };
  }, [client, key]);
  return useMemo(() => new Map([...looked, ...(known ?? [])]), [looked, known]);
}
