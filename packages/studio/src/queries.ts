// What the studio reads, and what a change makes stale.
//
// Every read is a query of TanStack Query, made with the client every other
// caller of the API uses. The keys are all written here, so that a change can
// say which reads it made stale, and nothing else is read again.
//
// A key is `[resource, kind, argument, visit]`:
//
//   resource   what of the server is read: `session`, `info`, `people`,
//              `tokens`, `grants`, `runs`, `identities`, `records`,
//              `documents`, `review`, `drafts`, `log`, `explain`, `names`
//   kind       `list` for many of them, `one` for one with what its screen
//              shows of it, or a word of the resource's own (`studio` for
//              what the studio reads once for every screen)
//   argument   what narrows a list, or the ID of the one
//   visit      which showing of a screen the read is of, which the hooks
//              here add: see `useVisit`
//
// `[resource]` alone, `keys.<resource>.all`, names every read of it. A screen's
// read may take in more than its own resource: an identity is read with its
// records, its constraints and its history. It has the key of the resource in
// its address, and `stale` lists every resource whose reads can show a change.

import { QueryClient, keepPreviousData, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import type { QueryKey } from "@tanstack/react-query";
import { useMemo, useState } from "react";

import type { Op, SpoolClient } from "./api.ts";
import { found } from "./find.ts";
import { identitiesOf, recordName, recordsOf, tail } from "./format.ts";
import { useStudio } from "./hooks.ts";

function resource<Name extends string>(name: Name) {
  return {
    /** Every read of it. */
    all: [name] as const,
    /** Many of them, as `filters` narrow and order them. */
    list: (filters: object = {}) => [name, "list", filters] as const,
    /** One of them, by its ID, with what its screen shows of it. */
    one: (id: string | number) => [name, "one", String(id)] as const,
  };
}

export const keys = {
  /** Who is signed in, and how people sign in. */
  session: ["session"],
  info: {
    all: ["info"],
    /** What the store holds, read once for every screen: the counts beside the screens' names. */
    studio: ["info", "studio"],
    /** The same with the latest runs, as the overview shows it. */
    overview: ["info", "overview"],
  },
  people: {
    all: ["people"],
    /** Everyone with an account, read once for every screen: the log names people by ID. */
    studio: ["people", "studio"],
    list: ["people", "list"],
  },
  tokens: resource("tokens"),
  /** The assistants people let in. */
  grants: resource("grants"),
  runs: resource("runs"),
  identities: {
    ...resource("identities"),
    /** What identities are called, by their IDs. */
    labels: (ids: string[]) => ["identities", "labels", ids] as const,
  },
  records: resource("records"),
  documents: resource("documents"),
  review: resource("review"),
  drafts: resource("drafts"),
  log: resource("log"),
  explain: {
    all: ["explain"],
    /** Why two records are one thing or are not. */
    pair: (a: string, b: string) => ["explain", "pair", a, b] as const,
  },
  /**
   * What records are called where a screen holds only their IDs. A record's
   * source and key never change, so no change makes a name stale.
   */
  names: (ids: string[]) => ["names", ids] as const,
} as const;

/** What a change makes stale: the keys to invalidate once it is made. */
export const stale = {
  /**
   * A change to what the store holds: a changeset that was committed (a
   * commit, an undo, a split, a merge, an approved draft), a draft that was
   * rejected, a review item that was dismissed. It moves records between
   * identities or settles what waited, and every read of the store can show
   * that: the counts, the lists, a thing's own screen with what is read
   * beside it, an explanation.
   */
  store: [
    keys.info.all,
    keys.runs.all,
    keys.identities.all,
    keys.records.all,
    keys.documents.all,
    keys.review.all,
    keys.drafts.all,
    keys.log.all,
    keys.explain.all,
  ],
  /** An account that was added or changed, which may be the reader's own. */
  people: [keys.session, keys.people.all],
  tokens: [keys.tokens.all],
  grants: [keys.grants.all],
} as const satisfies Record<string, readonly QueryKey[]>;

/**
 * Where what was read is kept. A screen is made anew for each address and
 * reads when it appears, and then again only when it is asked to: by
 * "Refresh", or by a change that made its read stale. So a read is not tried
 * again by itself when it fails, is not made again when the window is looked
 * at again or the network comes back, and is made whether or not the browser
 * believes it is online: the server may be on this machine. Nothing is kept
 * of a screen that is gone (`gcTime`).
 */
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        staleTime: 0,
        gcTime: 0,
        refetchOnWindowFocus: false,
        refetchOnReconnect: false,
        networkMode: "always",
      },
      mutations: { retry: false, networkMode: "always" },
    },
  });
}

let visits = 0;

/**
 * A number for one showing of a screen, which ends every key the screen
 * reads by. A screen is made anew for each address, and what it shows it
 * read itself: it never shows what an earlier visit read, not even while it
 * reads, because a reviewer decides from what is shown, and a change is
 * written against it. So no two showings share a read. A change still makes
 * the read of every showing stale by the parts of the key before this one.
 */
function useVisit(): number {
  return useState(() => ++visits)[0];
}

export type Loaded<T> = { status: "loading" } | { status: "failed"; error: unknown } | { status: "ready"; data: T };

const LOADING = { status: "loading" } as const;

/**
 * Reads when the screen appears, and again when what `key` names goes stale.
 * What was read stays on the screen until the new answer is there.
 */
export function useRead<T>(key: QueryKey, read: (client: SpoolClient) => Promise<T>): Loaded<T> {
  const { client } = useStudio();
  const visit = useVisit();
  const query = useQuery({
    queryKey: [...key, visit],
    queryFn: () => read(client),
    placeholderData: keepPreviousData,
  });
  if (query.status === "pending") return LOADING;
  if (query.status === "error") return { status: "failed", error: query.error };
  return { status: "ready", data: query.data };
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

/**
 * A list that is read a page at a time, as far as the reader asks. When it
 * goes stale, the pages that were read are read again.
 */
export function useReadPages<T>(
  key: QueryKey,
  read: (client: SpoolClient, after: string | number | undefined) => Promise<Page<T>>,
): Pages<T> {
  const { client } = useStudio();
  const visit = useVisit();
  const query = useInfiniteQuery({
    queryKey: [...key, visit],
    queryFn: ({ pageParam }) => read(client, pageParam),
    initialPageParam: undefined as string | number | undefined,
    getNextPageParam: (last) => last.next ?? undefined,
    placeholderData: keepPreviousData,
  });
  const items = useMemo(() => query.data?.pages.flatMap((page) => page.items), [query.data]);
  const loaded: Loaded<T[]> =
    query.status === "error"
      ? { status: "failed", error: query.error }
      : items === undefined
        ? LOADING
        : { status: "ready", data: items };
  return {
    loaded,
    more: query.hasNextPage,
    reading: query.isFetchingNextPage,
    readMore: () => {
      if (query.hasNextPage && !query.isFetchingNextPage) void query.fetchNextPage();
    },
  };
}

const NONE: Map<string, string> = new Map();

/** The most records one screen looks up to show their names in place of their IDs. */
const MOST_NAMES = 100;

/**
 * What to call the records a screen names only by ID, such as those in
 * the operations of a changeset: the source and key of each. `known` are
 * those the screen has read already. A record that is not looked up,
 * or not found, is shown by its ID.
 */
export function useRecordNames(ids: string[], known?: Map<string, string>): Map<string, string> {
  const { client } = useStudio();
  const visit = useVisit();
  const wanted = [...new Set(ids)].filter((id) => !known?.has(id)).slice(0, MOST_NAMES);
  const query = useQuery({
    queryKey: [...keys.names(wanted), visit],
    queryFn: async () => {
      // A name is a convenience: if one cannot be read, the ID stays.
      const records = await Promise.all(wanted.map((id) => found(client.record(id)).catch(() => null)));
      return new Map(records.flatMap((record) => (record ? [[record.id, recordName(record)]] : [])));
    },
    enabled: wanted.length > 0,
    placeholderData: keepPreviousData,
  });
  const looked = wanted.length > 0 ? (query.data ?? NONE) : NONE;
  return useMemo(() => new Map([...looked, ...(known ?? [])]), [looked, known]);
}

/** The most identities one screen looks up to show what they are called. */
const MOST_LABELS = 500;

/**
 * What the identities a screen names only by ID are called: the label of
 * each that has one, read in one call. An identity that is not looked up,
 * that was merged away or that no record names is shown by its ID.
 */
export function useIdentityLabels(ids: Array<string | null | undefined>): Map<string, string> {
  const { client } = useStudio();
  const visit = useVisit();
  const wanted = [...new Set(ids.filter((id): id is string => typeof id === "string" && id !== ""))]
    .sort()
    .slice(0, MOST_LABELS);
  // A label is a convenience: if it cannot be read, the ID stays.
  const query = useQuery({
    queryKey: [...keys.identities.labels(wanted), visit],
    queryFn: async () => {
      const page = await client.identities({ ids: wanted, limit: wanted.length });
      return new Map(page.identities.flatMap((identity) => (identity.label ? [[identity.id, identity.label]] : [])));
    },
    enabled: wanted.length > 0,
    placeholderData: keepPreviousData,
  });
  return query.data ?? NONE;
}

/**
 * What the records and the identities that operations name are called,
 * by ID. Two identities may be called the same, so each has the end of its
 * ID after its label.
 */
export function useOpNames(ops: Op[], known?: Map<string, string>): Map<string, string> {
  const names = useRecordNames(recordsOf(ops), known);
  const labels = useIdentityLabels(identitiesOf(ops));
  return useMemo(
    () => new Map([...names, ...[...labels].map(([id, label]) => [id, `${label} (…${tail(id)})`] as const)]),
    [names, labels],
  );
}
