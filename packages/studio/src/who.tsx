// Who made a change. The log names a person by the ID of their account,
// which does not change when their name does.

import { Code } from "./components/layout.tsx";
import { useStudio } from "./hooks.ts";

/** What an actor is called where `Who` shows it, for sorting by it. */
export function useCalled(): (actor: string) => string {
  const { people } = useStudio();
  return (actor) => (actor.startsWith("user:") ? people.get(actor.slice("user:".length))?.name : undefined) ?? actor;
}

export function Who({ actor }: { actor: string }) {
  const { people } = useStudio();
  const person = actor.startsWith("user:") ? people.get(actor.slice("user:".length)) : undefined;
  if (person === undefined) return <Code>{actor}</Code>;
  return (
    <span data-part="who" title={`${person.username} (${actor})`}>
      {person.name}
    </span>
  );
}
