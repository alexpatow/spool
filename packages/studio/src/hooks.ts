// What every screen has: the client, who is signed in, and what the store
// held when it was last asked. A screen reads with the hooks of `queries.ts`.

import { createContext, useContext, useMemo } from "react";

import type { Info, Person, SpoolClient } from "./api.ts";

export interface Studio {
  client: SpoolClient;
  /** Who is signed in. Their changes are recorded under their account. */
  me: Person;
  /** Everyone with an account, by its ID. */
  people: Map<string, Person>;
  /** Whether the server signs people in with Google. */
  google: boolean;
  /** What the store held when it was last asked: its entity types, and how much of each. */
  info: Info;
}

export const StudioContext = createContext<Studio | null>(null);

export function useStudio(): Studio {
  const studio = useContext(StudioContext);
  if (studio === null) throw new Error("the studio is not connected");
  return studio;
}

/** The names of the entity types the store has, which lists are narrowed by. */
export function useTypes(): string[] {
  const { info } = useStudio();
  return useMemo(() => info.store.entity_types.map((type) => type.name), [info]);
}
