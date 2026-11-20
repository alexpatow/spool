// A list as a table: its columns, the order of its rows, and the pages it
// is read in.

import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import type { Order } from "../address.ts";
import { Button } from "spool-ui/components/button";
import * as ShelfTable from "spool-ui/components/table";
import type { Pages } from "../queries.ts";
import { ArrowDownIcon, ArrowUpDownIcon, ArrowUpIcon } from "../icons.tsx";
import { colors, radius, spacing, typography } from "spool-ui/tokens.stylex";
import { shared } from "./shared.ts";
import { Load } from "./layout.tsx";

/** What a value of a row is sorted as. What is missing comes last, whichever way. */
export type Sortable = string | number | null | undefined;

export interface Column<T> {
  title: string;
  cell: (row: T) => ReactNode;
  /** Numbers are set to the right. */
  number?: boolean;
  /** Long text goes on to the next line where others stay on one. */
  wrap?: boolean;
  /**
   * What the server sorts a list read in pages by, when the table is
   * sorted by this column. Such a table is given `sorting`.
   */
  sort?: string;
  /** What a list read whole is sorted by in the page, when the table is sorted by this column. */
  value?: (row: T) => Sortable;
  /** Which way the first click sorts. Default: ascending, and descending for numbers. */
  first?: Order;
}

/** What a table is sorted by: a column's `sort`, or its title when it sorts in the page. */
export interface Sorted {
  by: string;
  order: Order;
}

/** The order of a list read in pages, which the server keeps, and what asks for another. */
export interface Sorting extends Sorted {
  choose: (sorted: Sorted) => void;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

function compare(a: Sortable, b: Sortable, order: Order): number {
  const missing = (value: Sortable) => value === null || value === undefined || value === "";
  if (missing(a) || missing(b)) return Number(missing(a)) - Number(missing(b));
  const way = order === "asc" ? 1 : -1;
  if (typeof a === "number" && typeof b === "number") return (a - b) * way;
  return collator.compare(String(a), String(b)) * way;
}

/** The rows in the order of a column. Rows that sort the same keep the order they came in. */
export function sortRows<T>(rows: T[], value: (row: T) => Sortable, order: Order): T[] {
  return rows
    .map((row, at) => ({ row, at, value: value(row) }))
    .sort((a, b) => compare(a.value, b.value, order) || a.at - b.at)
    .map(({ row }) => row);
}

const SAID = { asc: "ascending", desc: "descending" } as const;

function firstOrder<T>(column: Column<T>): Order {
  return column.first ?? (column.number ? "desc" : "asc");
}

/** What a column's header is, when the table can be sorted by it: a button that sorts, and which way it is sorted. */
function SortHeader({ title, order, onSort }: { title: string; order: Order | null; onSort: () => void }) {
  const Icon = order === "asc" ? ArrowUpIcon : order === "desc" ? ArrowDownIcon : ArrowUpDownIcon;
  return (
    <button
      type="button"
      data-part="sort"
      onClick={onSort}
      {...stylex.props(styles.sortButton, order !== null && styles.sortButtonOn)}
    >
      {title}
      <Icon {...stylex.props(styles.sortIcon, order === null && styles.sortIconOff)} />
    </button>
  );
}

export function Table<T>({
  columns,
  rows,
  rowKey,
  empty,
  label,
  sorting,
  initial,
}: {
  columns: Array<Column<T>>;
  rows: T[];
  rowKey: (row: T) => string | number;
  /** What to say when there is nothing to list. */
  empty: string;
  /** What the table lists, for a reader who cannot see what is above it. */
  label?: string;
  /** How the server sorted the rows, for a list read in pages. Without it the table sorts its rows itself. */
  sorting?: Sorting;
  /**
   * The order the table shows its rows in at first when it sorts them
   * itself, by a column's title. Default: as they came.
   */
  initial?: Sorted;
}) {
  const [own, setOwn] = useState<Sorted | null>(initial ?? null);
  const sorted: Sorted | null = sorting ?? own;
  const choose = sorting?.choose ?? setOwn;
  const keyOf = (column: Column<T>) => (sorting ? column.sort : column.value ? column.title : undefined);
  const by = sorting ? undefined : columns.find((column) => column.title === own?.by)?.value;
  const shown = by && own ? sortRows(rows, by, own.order) : rows;
  if (rows.length === 0) {
    return (
      <div data-part="empty" {...stylex.props(shared.panel, styles.empty)}>
        {empty}
      </div>
    );
  }
  return (
    <div {...stylex.props(shared.panel)}>
      <ShelfTable.Root aria-label={label}>
        <ShelfTable.Header>
          <ShelfTable.Row style={styles.headRow}>
            {columns.map((column) => {
              const key = keyOf(column);
              const order = key !== undefined && sorted?.by === key ? sorted.order : null;
              return (
                <ShelfTable.Head
                  key={column.title}
                  aria-sort={key === undefined ? undefined : order === null ? "none" : SAID[order]}
                  style={[
                    styles.head,
                    column.number && styles.number,
                    column.title === "" && styles.tick,
                    key !== undefined && styles.sortable,
                  ]}
                >
                  {key === undefined ? (
                    column.title
                  ) : (
                    <SortHeader
                      title={column.title}
                      order={order}
                      onSort={() =>
                        choose({
                          by: key,
                          order: order === null ? firstOrder(column) : order === "asc" ? "desc" : "asc",
                        })
                      }
                    />
                  )}
                </ShelfTable.Head>
              );
            })}
          </ShelfTable.Row>
        </ShelfTable.Header>
        <ShelfTable.Body>
          {shown.map((row) => (
            <ShelfTable.Row key={rowKey(row)} style={styles.bodyRow}>
              {columns.map((column) => (
                <ShelfTable.Cell
                  key={column.title}
                  style={[styles.cell, column.number && styles.number, column.wrap && styles.wrap]}
                >
                  {column.cell(row)}
                </ShelfTable.Cell>
              ))}
            </ShelfTable.Row>
          ))}
        </ShelfTable.Body>
      </ShelfTable.Root>
    </div>
  );
}

/** The button under a list that reads its next page. */
export function More({ pages }: { pages: Pick<Pages<unknown>, "more" | "reading" | "readMore"> }) {
  if (!pages.more) return null;
  return (
    <div {...stylex.props(shared.more)}>
      <Button variant="outline" size="sm" onClick={pages.readMore} disabled={pages.reading}>
        {pages.reading ? "Loading…" : "Show more"}
      </Button>
    </div>
  );
}

/** A list read in pages, with the button that reads the next one. */
export function PagedTable<T>({
  pages,
  ...table
}: {
  pages: Pages<T>;
  columns: Array<Column<T>>;
  rowKey: (row: T) => string | number;
  empty: string;
  label?: string;
  sorting?: Sorting;
}) {
  return (
    <Load state={pages.loaded}>
      {(rows) => (
        <>
          <Table rows={rows} {...table} />
          <More pages={pages} />
        </>
      )}
    </Load>
  );
}

const styles = stylex.create({
  empty: {
    padding: spacing["6"],
    backgroundColor: colors.card,
    color: colors.mutedForeground,
    textAlign: "center",
  },
  headRow: {
    backgroundColor: colors.card,
  },
  bodyRow: {
    backgroundColor: { default: "transparent", ":hover": colors.card },
    borderBottomWidth: { default: 1, ":last-child": 0 },
  },
  head: {
    paddingInline: spacing["3"],
    fontSize: typography.fontSizeXs,
    height: "2.25rem",
  },
  cell: {
    paddingBlock: spacing["2"],
    paddingInline: spacing["3"],
  },
  number: {
    fontVariantNumeric: "tabular-nums",
    textAlign: "end",
  },
  tick: {
    width: "1.5rem",
  },
  sortable: {
    paddingInline: spacing["1.5"],
  },
  sortButton: {
    borderRadius: radius.sm,
    borderStyle: "none",
    gap: spacing["1"],
    margin: 0,
    paddingBlock: spacing["1"],
    paddingInline: spacing["1.5"],
    alignItems: "center",
    backgroundColor: { default: "transparent", ":hover": colors.accent },
    color: { default: "inherit", ":hover": colors.foreground },
    cursor: "pointer",
    display: "inline-flex",
    font: "inherit",
    outlineColor: colors.ring,
    outlineOffset: 1,
    outlineStyle: { default: "none", ":focus-visible": "solid" },
    outlineWidth: 2,
    whiteSpace: "nowrap",
  },
  sortButtonOn: {
    color: colors.foreground,
  },
  sortIcon: {
    flexShrink: 0,
  },
  sortIconOff: {
    opacity: 0.5,
  },
  // A cell of running text wraps between words, and keeps the width to read
  // them in: on a narrow screen the table scrolls sideways before a name
  // is broken into letters.
  wrap: {
    minWidth: "10rem",
    overflowWrap: "break-word",
    whiteSpace: "normal",
  },
});
