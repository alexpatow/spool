// The review items: what the engine could not decide by itself, each
// with all a reviewer decides it from.

import * as stylex from "@stylexjs/stylex";
import { getRouteApi } from "@tanstack/react-router";
import { useState } from "react";
import type { ReactNode } from "react";

import { Done } from "../../act.tsx";
import type { ReviewEntry, ReviewStatus } from "../../api.ts";
import { Load, Page } from "../../components/layout.tsx";
import { sortRows } from "../../components/table.tsx";
import { Labelled, Select } from "../../components/forms.tsx";
import { Choose, Filters, useNarrow } from "../../components/filters.tsx";
import { Button } from "spool-ui/components/button";
import { useStudio, useTypes } from "../../hooks.ts";
import { InboxIcon } from "../../icons.tsx";
import { keys, useRead } from "../../queries.ts";
import { colors, radius, spacing } from "spool-ui/tokens.stylex";
import { Case } from "./case.tsx";

const STATUSES: ReviewStatus[] = ["open", "resolved", "dismissed", "superseded"];

/** How many items are shown with all a reviewer decides them from, before the reader asks for more. */
const AT_ONCE = 10;

/** The orders the items can be read in. The server gives them oldest first. */
const ORDERS = [
  { value: "oldest", label: "oldest first" },
  { value: "newest", label: "newest first" },
  { value: "records", label: "most records first" },
  { value: "kind", label: "kind" },
];

function ordered(items: ReviewEntry[], order: string): ReviewEntry[] {
  switch (order) {
    case "newest":
      return sortRows(items, (item) => item.id, "desc");
    case "records":
      return sortRows(items, (item) => item.records.length, "desc");
    case "kind":
      return sortRows(items, (item) => item.kind, "asc");
    default:
      return items;
  }
}

const list = getRouteApi("/review/");

export function ReviewItems() {
  const search = list.useSearch();
  const { type } = search;
  const status = search.status ?? "open";
  const order = search.sort ?? "oldest";
  const { info } = useStudio();
  const types = useTypes();
  const narrow = useNarrow();
  const [shown, setShown] = useState(AT_ONCE);
  // An item that was decided leaves the list of those that are open, so
  // what came of it is said above the list.
  const [done, setDone] = useState<ReactNode>(null);
  const filters = { status: status === "all" ? undefined : (status as ReviewStatus), entity_type: type };
  const items = useRead(keys.review.list(filters), (client) => client.reviewItems(filters));
  const waiting = info.store.entity_types
    .filter((entity) => type === undefined || entity.name === type)
    .reduce((sum, entity) => sum + entity.open_review_items, 0);
  return (
    <Page title="Review">
      <div {...stylex.props(styles.narrow)}>
        <Choose
          label="Status"
          choices={[...STATUSES, "all"].map((value) => ({
            label: value === "open" ? "Open" : value[0]!.toUpperCase() + value.slice(1),
            leads: { to: "/review", search: { ...search, status: value === "open" ? undefined : value } },
            current: status === value,
            count: value === "open" ? waiting : undefined,
          }))}
        />
        <div {...stylex.props(styles.listing)}>
          {types.length > 1 ? (
            <Filters
              query={search}
              narrow={narrow}
              selects={[{ name: "type", label: "Entity type", choices: types.map((value) => ({ value })) }]}
            />
          ) : null}
          <Labelled label="Order">
            <Select
              value={order}
              onChange={(value) => narrow({ sort: value === "oldest" ? undefined : value })}
              choices={ORDERS}
            />
          </Labelled>
        </div>
      </div>
      <Done>{done}</Done>
      <Load state={items}>
        {(items) =>
          items.length === 0 ? (
            <div data-part="empty" {...stylex.props(styles.nothing)}>
              <InboxIcon {...stylex.props(styles.nothingIcon)} />
              <strong>{status === "open" ? "No open review items." : "No review items."}</strong>
            </div>
          ) : (
            <>
              {ordered(items, order)
                .slice(0, shown)
                .map((item) => (
                  <Case key={item.id} id={item.id} linked done={setDone} />
                ))}
              {items.length > shown ? (
                <div {...stylex.props(styles.more)}>
                  <Button variant="outline" size="sm" onClick={() => setShown(shown + AT_ONCE)}>
                    Show {Math.min(AT_ONCE, items.length - shown)} more of {items.length}
                  </Button>
                </div>
              ) : null}
            </>
          )
        }
      </Load>
    </Page>
  );
}

const styles = stylex.create({
  narrow: {
    gap: spacing["4"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "space-between",
  },
  listing: {
    gap: spacing["3"],
    alignItems: "flex-end",
    display: "flex",
    flexWrap: "wrap",
  },
  nothing: {
    borderColor: colors.border,
    borderRadius: radius.lg,
    borderStyle: "dashed",
    borderWidth: 1,
    gap: spacing["1.5"],
    paddingBlock: "3rem",
    paddingInline: spacing["6"],
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    textAlign: "center",
  },
  nothingIcon: {
    color: colors.mutedForeground,
    fontSize: "1.5rem",
    marginBottom: spacing["2"],
  },
  more: {
    display: "flex",
    justifyContent: "center",
  },
});
