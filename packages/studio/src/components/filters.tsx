// What narrows a list and puts it in order, kept in the address, and what
// a reader picks out of one.

import * as stylex from "@stylexjs/stylex";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import type { ReactNode } from "react";

import type { Order, QueryValue } from "../address.ts";
import { Button } from "spool-ui/components/button";
import { Checkbox } from "spool-ui/components/checkbox";
import { Input } from "spool-ui/components/input";
import { colors, radius, spacing, typography } from "spool-ui/tokens.stylex";
import { shared } from "./shared.ts";
import { Quiet } from "./layout.tsx";
import type { Sorted, Sorting } from "./table.tsx";
import { Onward, ScreenLink } from "./refs.tsx";
import type { Leads } from "./refs.tsx";
import { Flag, Labelled, Select } from "./forms.tsx";
import type { Choice } from "./forms.tsx";

export interface Choosing {
  label: string;
  leads: Leads;
  current: boolean;
  /** How many there are of it, when the screen knows. */
  count?: number;
}

/** The ways one list can be narrowed, of which one is chosen. Each is a link, so the choice is in the address. */
export function Choose({ label, choices }: { label: string; choices: Choosing[] }) {
  return (
    <ul aria-label={label} {...stylex.props(styles.choose)}>
      {choices.map((choice) => (
        <li key={choice.label} {...stylex.props(styles.choiceItem)}>
          <ScreenLink
            {...choice.leads}
            current={choice.current ? "true" : undefined}
            {...stylex.props(styles.choice, choice.current && styles.choiceCurrent)}
          >
            {choice.label}
            {choice.count !== undefined ? (
              <span data-part="count" {...stylex.props(shared.count)}>
                {choice.count}
              </span>
            ) : null}
          </ScreenLink>
        </li>
      ))}
    </ul>
  );
}

/** Goes to the screen that is shown, with some of its filters changed. */
export type Narrow = (filters: Record<string, QueryValue>) => void;

/**
 * The way to change the filters of the screen that is shown. The others
 * stay as the address has them, and what is changed comes after them.
 */
export function useNarrow(): Narrow {
  const navigate = useNavigate();
  // Which filters a screen has is its route's to say: the route reads them out of the address again.
  return (filters) => void navigate({ to: ".", search: (before) => ({ ...before, ...filters }) as never });
}

/**
 * The order of a list read in pages, kept in the address as `sort` and
 * `order` so that a sorted list can be linked to. `own` is the order it has
 * when the address names none.
 */
export function sortingOf(search: { sort?: string; order?: Order }, narrow: Narrow, own: Sorted): Sorting {
  return {
    by: search.sort ?? own.by,
    order: search.order ?? (search.sort === undefined ? own.order : "asc"),
    choose: ({ by, order }) => narrow({ sort: by, order }),
  };
}

/**
 * The filters of a list. Each change goes to the address, so that a list
 * as it was narrowed can be linked to.
 */
export function Filters({
  query,
  narrow,
  search,
  selects = [],
  texts = [],
  flags = [],
}: {
  /** The filters as the address has them. */
  query: Record<string, QueryValue>;
  narrow: Narrow;
  /** A box for words to look for, which is the first thing in the row. */
  search?: { name: string; label: string; placeholder: string };
  selects?: Array<{ name: string; label: string; choices: Choice[] }>;
  texts?: Array<{ name: string; label: string }>;
  flags?: Array<{ name: string; label: string }>;
}) {
  const said = (name: string) => String(query[name] ?? "");
  const typed = [...(search ? [search] : []), ...texts];
  return (
    <form
      role="search"
      {...stylex.props(styles.filters)}
      onSubmit={(event) => {
        event.preventDefault();
        const entered = new FormData(event.currentTarget);
        const values: Record<string, QueryValue> = {};
        for (const { name } of typed) values[name] = String(entered.get(name) ?? "").trim();
        narrow(values);
      }}
    >
      {search ? (
        <Labelled label={search.label}>
          <Input
            name={search.name}
            type="search"
            defaultValue={said(search.name)}
            placeholder={search.placeholder}
            spellCheck={false}
            autoComplete="off"
            style={styles.searchBox}
          />
        </Labelled>
      ) : null}
      {selects.map(({ name, label, choices }) => (
        <Labelled key={name} label={label}>
          <Select value={said(name)} onChange={(value) => narrow({ [name]: value })} choices={choices} any="any" />
        </Labelled>
      ))}
      {texts.map(({ name, label }) => (
        <Labelled key={name} label={label}>
          <Input name={name} defaultValue={said(name)} spellCheck={false} autoComplete="off" />
        </Labelled>
      ))}
      {flags.map(({ name, label }) => (
        <Flag
          key={name}
          label={label}
          checked={query[name] === true}
          onChange={(checked) => narrow({ [name]: checked })}
        />
      ))}
      {typed.length > 0 ? (
        <Button type="submit" variant="outline">
          {search && texts.length === 0 ? "Search" : "Apply"}
        </Button>
      ) : null}
    </form>
  );
}

/**
 * Keeps which records of a list the reader ticked, in the order they
 * were ticked. With `most`, ticking one more lets go of the earliest.
 */
export function useTicks(most?: number): {
  chosen: string[];
  tick: (id: string) => ReactNode;
} {
  const [chosen, setChosen] = useState<string[]>([]);
  const toggle = (id: string) =>
    setChosen((before) =>
      before.includes(id)
        ? before.filter((other) => other !== id)
        : [...before, id].slice(-(most ?? before.length + 1)),
    );
  return {
    chosen,
    tick: (id) => (
      <Checkbox aria-label={`Select record ${id}`} checked={chosen.includes(id)} onCheckedChange={() => toggle(id)} />
    ),
  };
}

/** The link that explains two ticked records, or what to tick to get it. */
export function ExplainTicked({ chosen }: { chosen: string[] }) {
  return chosen.length === 2 ? (
    <Onward to="/explain" search={{ a: chosen[0], b: chosen[1] }}>
      Explain selected
    </Onward>
  ) : (
    <Quiet>Select two to explain.</Quiet>
  );
}

/**
 * Keeps which two records of a list the reader ticked, and gives the
 * link that explains them.
 */
export function usePair(): {
  tick: (id: string) => ReactNode;
  explain: ReactNode;
} {
  const { chosen, tick } = useTicks(2);
  return { tick, explain: <ExplainTicked chosen={chosen} /> };
}

const styles = stylex.create({
  choose: {
    margin: 0,
    borderRadius: radius.md,
    gap: "0.125rem",
    padding: "0.1875rem",
    alignSelf: "flex-start",
    backgroundColor: colors.muted,
    display: "inline-flex",
    flexWrap: "wrap",
    listStyleType: "none",
  },
  choiceItem: {
    display: "flex",
  },
  choice: {
    borderRadius: radius.sm,
    gap: spacing["1.5"],
    paddingBlock: spacing["1"],
    paddingInline: spacing["2.5"],
    alignItems: "center",
    color: { default: colors.mutedForeground, ":hover": colors.foreground },
    display: "inline-flex",
    fontWeight: typography.fontWeightMedium,
    textDecorationLine: "none",
  },
  choiceCurrent: {
    backgroundColor: colors.background,
    boxShadow: `inset 0 0 0 1px ${colors.border}`,
    color: colors.foreground,
  },
  filters: {
    gap: spacing["3"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
  },
  searchBox: {
    width: "20rem",
    maxWidth: "100%",
  },
});
