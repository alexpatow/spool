// Finding a thing from what a person has to hand: some words of a name, an
// ID out of a log line, or the source and key of a row. It opens over any
// screen, and what it finds is one press away.

import * as stylex from "@stylexjs/stylex";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";

import type { IdentitySummary, ObservationRecord } from "./api.ts";
import { Source, Sources, message } from "./components.tsx";
import * as Command from "./components/ui/command";
import { findScreen, isReference } from "./find.ts";
import { count, observationName, short } from "./format.ts";
import { useStudio } from "./hooks.ts";
import { ArrowRightIcon, IdentityIcon, RecordIcon, SearchIcon } from "./icons.tsx";
import { href } from "./router.ts";
import { colors, spacing, typography } from "./styles/shelf/tokens.stylex";

export interface Place {
  path: string;
  label: string;
  icon: ReactNode;
}

interface Found {
  /** Tells it from every other line of the list. */
  value: string;
  /** What the line is found by, which is the words typed: the server did the finding. */
  label: string;
  line: ReactNode;
  go: () => void;
}

interface Group {
  value: string;
  items: Found[];
}

/** The most of each kind that are listed. The lists say where the rest are. */
const MOST = 6;

/** How long the reader has to stop typing before the server is asked. */
const PAUSE_MS = 160;

interface Results {
  words: string;
  identities: IdentitySummary[];
  observations: ObservationRecord[];
}

export function Palette({
  open,
  onOpenChange,
  places,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  places: Place[];
}) {
  const { client } = useStudio();
  const [typed, setTyped] = useState("");
  const [results, setResults] = useState<Results | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const words = typed.trim();
  // Which search is the latest: an answer to an earlier one is not shown.
  const asked = useRef(0);

  useEffect(() => {
    if (!open) {
      setTyped("");
      setResults(null);
      setProblem(null);
    }
  }, [open]);

  useEffect(() => {
    const mine = ++asked.current;
    setProblem(null);
    if (words === "") {
      setResults(null);
      setBusy(false);
      return;
    }
    setBusy(true);
    const timer = setTimeout(() => {
      Promise.all([client.identities({ q: words, limit: MOST }), client.observations({ q: words, limit: MOST })]).then(
        ([identities, observations]) => {
          if (asked.current !== mine) return;
          setBusy(false);
          setResults({ words, identities: identities.identities, observations: observations.observations });
        },
        (error: unknown) => {
          if (asked.current !== mine) return;
          setBusy(false);
          setProblem(message(error));
        },
      );
    }, PAUSE_MS);
    return () => clearTimeout(timer);
  }, [client, words]);

  const groups = useMemo((): Group[] => {
    const go = (address: string) => () => {
      window.location.hash = address;
      onOpenChange(false);
    };
    const list: Group[] = [];
    if (words !== "" && isReference(words)) {
      list.push({
        value: "Open",
        items: [
          {
            value: `open:${words}`,
            label: words,
            line: (
              <>
                <ArrowRightIcon />
                <span>
                  Open what <code {...stylex.props(styles.code)}>{words}</code> names
                </span>
              </>
            ),
            go: () => {
              setBusy(true);
              findScreen(client, words).then(
                (screen) => {
                  setBusy(false);
                  if (screen === null) setProblem(`Nothing here is named ${words}.`);
                  else go(screen)();
                },
                (error: unknown) => {
                  setBusy(false);
                  setProblem(message(error));
                },
              );
            },
          },
        ],
      });
    }
    if (results !== null && results.words === words) {
      if (results.identities.length > 0) {
        list.push({
          value: "Identities",
          items: [
            ...results.identities.map((identity) => ({
              value: `identity:${identity.id}`,
              label: words,
              line: (
                <>
                  <IdentityIcon />
                  <span {...stylex.props(styles.name)}>{identity.label ?? short(identity.id)}</span>
                  <span {...stylex.props(styles.aside)}>
                    {identity.entity_type} · {count(identity.members, "record")}
                  </span>
                  <Sources sources={identity.sources} />
                </>
              ),
              go: go(href(["identities", identity.id])),
            })),
            ...(results.identities.length === MOST
              ? [
                  {
                    value: "identities:all",
                    label: words,
                    line: (
                      <>
                        <SearchIcon />
                        <span>Every identity that holds “{words}”</span>
                      </>
                    ),
                    go: go(href(["identities"], { q: words })),
                  },
                ]
              : []),
          ],
        });
      }
      if (results.observations.length > 0) {
        list.push({
          value: "Records",
          items: [
            ...results.observations.map((observation) => ({
              value: `observation:${observation.id}`,
              label: words,
              line: (
                <>
                  <RecordIcon />
                  <span {...stylex.props(styles.name)}>{observation.label ?? observationName(observation)}</span>
                  <span {...stylex.props(styles.aside)}>{observation.key ?? short(observation.id)}</span>
                  <Source name={observation.source} />
                </>
              ),
              go: go(href(["observations", observation.id])),
            })),
            ...(results.observations.length === MOST
              ? [
                  {
                    value: "observations:all",
                    label: words,
                    line: (
                      <>
                        <SearchIcon />
                        <span>Every record that holds “{words}”</span>
                      </>
                    ),
                    go: go(href(["observations"], { q: words })),
                  },
                ]
              : []),
          ],
        });
      }
    }
    const screens = places.filter((place) => place.label.toLowerCase().includes(words.toLowerCase()));
    if (screens.length > 0) {
      list.push({
        value: "Screens",
        items: screens.map((place) => ({
          value: `screen:${place.path}`,
          label: words,
          line: (
            <>
              {place.icon}
              <span>{place.label}</span>
            </>
          ),
          go: go(href(place.path === "" ? [] : [place.path])),
        })),
      });
    }
    return list;
  }, [client, onOpenChange, places, results, words]);

  const waiting = busy || (words !== "" && results?.words !== words);

  return (
    <Command.Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Find"
      description="Find an identity or a record by words it holds, by its ID, or by its source and key."
      style={styles.dialog}
    >
      <Command.Root items={groups} filter={null} value={typed} onValueChange={(value) => setTyped(value)}>
        <Command.Input
          aria-label="Find by words, by ID, or by source and key"
          placeholder="A name, an ID, or source:key…"
          spellCheck={false}
          autoComplete="off"
        />
        {problem ? (
          <div role="alert" {...stylex.props(styles.problem)}>
            {problem}
          </div>
        ) : null}
        <Command.List aria-label="What was found">
          {(group: Group) => (
            <Command.Group key={group.value} items={group.items}>
              <Command.GroupLabel>{group.value}</Command.GroupLabel>
              <Command.Collection>
                {(item: Found) => (
                  <Command.Item key={item.value} value={item} onClick={item.go} style={styles.item}>
                    {item.line}
                  </Command.Item>
                )}
              </Command.Collection>
            </Command.Group>
          )}
        </Command.List>
        {groups.length === 0 ? (
          <div role="status" {...stylex.props(styles.nothing)}>
            {waiting ? "Looking…" : `Nothing here holds “${words}”.`}
          </div>
        ) : null}
        <div {...stylex.props(styles.foot)}>
          Every word has to be in a record: in one of its values, its source or its key.
        </div>
      </Command.Root>
    </Command.Dialog>
  );
}

const styles = stylex.create({
  dialog: {
    maxWidth: "40rem",
    top: "18%",
    transform: "translateX(-50%)",
  },
  item: {
    minWidth: 0,
  },
  name: {
    fontWeight: typography.fontWeightMedium,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  aside: {
    color: colors.mutedForeground,
    flexGrow: 1,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  code: {
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
  },
  nothing: {
    paddingBlock: spacing["6"],
    color: colors.mutedForeground,
    textAlign: "center",
  },
  problem: {
    paddingBlock: spacing["2"],
    paddingInline: spacing["3"],
    color: colors.destructiveText,
  },
  foot: {
    borderTopColor: colors.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    paddingBlock: spacing["2"],
    paddingInline: spacing["3"],
    color: colors.mutedForeground,
    fontSize: typography.fontSizeXs,
    lineHeight: typography.lineHeightXs,
  },
});
