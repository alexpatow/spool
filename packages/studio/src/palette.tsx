// Finding a thing from what a person has to hand: some words of a name, an
// ID out of a log line, or the source and key of a row. It opens over any
// screen, and what it finds is one press away.

import * as stylex from "@stylexjs/stylex";
import { useNavigate } from "@tanstack/react-router";
import type { LinkProps } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";

import type { IdentitySummary, RecordEntry } from "./api.ts";
import { message } from "./components/layout.tsx";
import { SCREENS, Source, Sources } from "./components/refs.tsx";
import * as Command from "spool-ui/components/command";
import { findScreen, isReference } from "./find.ts";
import { count, identityMatch, recordName, short } from "./format.ts";
import { useStudio } from "./hooks.ts";
import { ArrowRightIcon, IdentityIcon, RecordIcon, SearchIcon } from "./icons.tsx";
import { colors, spacing, typography } from "spool-ui/tokens.stylex";

/** A screen that is not of one thing: the overview, or a list. */
export interface Place {
  to:
    | "/"
    | "/review"
    | "/drafts"
    | "/identities"
    | "/records"
    | "/documents"
    | "/explain"
    | "/runs"
    | "/log"
    | "/people"
    | "/tokens";
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
  records: RecordEntry[];
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
  const navigate = useNavigate();
  const [typed, setTyped] = useState("");
  const [results, setResults] = useState<Results | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const words = typed.trim();
  // Which search is the latest: an answer to an earlier one is not shown.
  const asked = useRef(0);

  useEffect(() => {
    if (!open) {
      // A palette that closes forgets what was typed into it, for the next time it opens.
      // oxlint-disable-next-line react/set-state-in-effect
      setTyped("");
      setResults(null);
      setProblem(null);
    }
  }, [open]);

  useEffect(() => {
    const mine = ++asked.current;
    // Each search starts clean: the problem shown was the last search's.
    // oxlint-disable-next-line react/set-state-in-effect
    setProblem(null);
    if (words === "") {
      setResults(null);
      setBusy(false);
      return;
    }
    setBusy(true);
    const timer = setTimeout(() => {
      Promise.all([client.identities({ q: words, limit: MOST }), client.records({ q: words, limit: MOST })]).then(
        ([identities, records]) => {
          if (asked.current !== mine) return;
          setBusy(false);
          setResults({ words, identities: identities.identities, records: records.records });
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
    const go = (leads: LinkProps) => () => {
      void navigate(leads);
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
                  Go to <code {...stylex.props(styles.code)}>{words}</code>
                </span>
              </>
            ),
            go: () => {
              setBusy(true);
              findScreen(client, words).then(
                (found) => {
                  setBusy(false);
                  if (found === null) setProblem(`${words} not found.`);
                  else go({ to: SCREENS[found.kind], params: { id: found.id } })();
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
                    {[identityMatch(identity.match), identity.entity_type, count(identity.members, "record")]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
                  <Sources sources={identity.sources} />
                </>
              ),
              go: go({ to: "/identities/$id", params: { id: identity.id } }),
            })),
            ...(results.identities.length === MOST
              ? [
                  {
                    value: "identities:all",
                    label: words,
                    line: (
                      <>
                        <SearchIcon />
                        <span>All identities matching “{words}”</span>
                      </>
                    ),
                    go: go({ to: "/identities", search: { q: words } }),
                  },
                ]
              : []),
          ],
        });
      }
      if (results.records.length > 0) {
        list.push({
          value: "Records",
          items: [
            ...results.records.map((record) => ({
              value: `record:${record.id}`,
              label: words,
              line: (
                <>
                  <RecordIcon />
                  <span {...stylex.props(styles.name)}>{record.label ?? recordName(record)}</span>
                  <span {...stylex.props(styles.aside)}>{record.key ?? short(record.id)}</span>
                  <Source name={record.source} />
                </>
              ),
              go: go({ to: "/records/$id", params: { id: record.id } }),
            })),
            ...(results.records.length === MOST
              ? [
                  {
                    value: "records:all",
                    label: words,
                    line: (
                      <>
                        <SearchIcon />
                        <span>All records matching “{words}”</span>
                      </>
                    ),
                    go: go({ to: "/records", search: { q: words } }),
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
          value: `screen:${place.to}`,
          label: words,
          line: (
            <>
              {place.icon}
              <span>{place.label}</span>
            </>
          ),
          go: go({ to: place.to }),
        })),
      });
    }
    return list;
  }, [client, navigate, onOpenChange, places, results, words]);

  const waiting = busy || (words !== "" && results?.words !== words);

  return (
    <Command.Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Search"
      description="Search identities and records."
      style={styles.dialog}
    >
      <Command.Root items={groups} filter={null} value={typed} onValueChange={(value) => setTyped(value)}>
        <Command.Input
          aria-label="Search"
          placeholder="Name, ID, or source:key"
          spellCheck={false}
          autoComplete="off"
        />
        {problem ? (
          <div role="alert" {...stylex.props(styles.problem)}>
            {problem}
          </div>
        ) : null}
        <Command.List aria-label="Results">
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
            {waiting ? "Searching…" : `No results for “${words}”.`}
          </div>
        ) : null}
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
});
