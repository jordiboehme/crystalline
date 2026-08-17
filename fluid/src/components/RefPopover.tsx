/**
 * A counted chip that opens onto the things it counts.
 *
 * The shape a reference set takes once it stops being small: the label and the
 * number are the whole of what is on the page, and the entries behind them are
 * fetched a page at a time, only when somebody asks, with a filter box for the
 * case where the page that matters is not the first one. An engram a thousand
 * engrams point at costs one number here rather than a thousand rows.
 *
 * Written as a primitive rather than as part of the backlinks panel on purpose:
 * `fetchPage` is a plain `(page, q) => Promise<RefPageResult>`, so the folder
 * overflow ("and 240 more") and the graph's hidden-node count can adopt this
 * without either of them learning anything about references. Nothing here knows
 * what it is listing; the caller names it.
 *
 * A popover rather than a dropdown menu, which is the app's other floating
 * surface: a menu owns its arrow keys and its typeahead, and both fight a text
 * input inside it. The surface is drawn with the shared menu classes anyway, so
 * a reader who has learned what a floating panel looks like here meets the same
 * one.
 */

import { Popover } from "radix-ui";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { Link } from "react-router";

import { plural } from "../format";
import { MENU_CLASSES } from "./menu";
import { FOCUS_RING } from "./primitives";

/** One row of a page: what it is called, where it goes, what names it. */
export interface RefRow {
  /** Stable within a page set, and used as the React key. */
  key: string;
  /** The link text. */
  title: string;
  /** Where the row leads. */
  href: string;
  /** The muted second line: a path, a domain, whatever locates it. */
  detail: string;
  /**
   * An extra class for the row, for a retired entry and the like. Spelled with
   * `undefined` in the type because this project checks optional properties
   * exactly: a caller that computes the class and gets nothing must still be
   * able to hand the nothing over.
   */
  className?: string | undefined;
}

/** One page, as a fetcher answers it. */
export interface RefPageResult {
  /** How many entries match the filter in total, exactly. */
  total: number;
  /** This page's rows. */
  rows: RefRow[];
  /** Whether a page follows this one. */
  hasMore: boolean;
}

export interface RefPopoverProps {
  /** The chip's label: a relation type, "more", whatever is being counted. */
  label: string;
  /** The number on the chip. */
  count: number;
  /**
   * One page of entries. Called with a one-based page and the current filter
   * text (empty for none), and never called until the popover is opened.
   */
  fetchPage: (page: number, q: string) => Promise<RefPageResult>;
  /** The singular of what is being listed, for the count line. */
  noun?: string;
  /** Its plural. */
  nounPlural?: string;
  /** What the chip is called to a screen reader, when the label alone is thin. */
  ariaLabel?: string;
}

/** How long a typed filter settles before it is sent. */
const DEBOUNCE_MS = 250;

/**
 * The chip's two faces, each a whole class string.
 *
 * Two faces rather than accent utilities layered onto one, for the reason
 * `TOGGLE` in the primitives spells out: Tailwind resolves same-specificity
 * conflicts by emission order, so a `text-accent-800` written after a
 * `text-slate-700` in the class attribute would still lose to it and the open
 * chip would never change color. Both pairs are the proven `Chip` pairs -
 * neutral is `bg-slate-100 text-slate-700` (10.87:1 light) and its dark
 * `bg-slate-800 text-slate-300` (9.79:1), open is `bg-accent-100
 * text-accent-800` (7.06:1) and `bg-accent-950 text-accent-300` (8.98:1) -
 * carried over unchanged from the chip that is already on these screens.
 */
const CHIP = {
  closed:
    "bg-slate-100 text-slate-700 hover:bg-slate-200 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-700",
  open: "bg-accent-100 text-accent-800 hover:bg-accent-200 dark:bg-accent-950 dark:text-accent-300 dark:hover:bg-accent-900",
} as const;

/** The geometry both faces share, chip-shaped and button-sized. */
const CHIP_SHAPE = `inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-caption ${FOCUS_RING}`;

/** What the list is doing right now. */
type Phase = "idle" | "loading" | "ready" | "failed";

export function RefPopover({
  label,
  count,
  fetchPage,
  noun = "reference",
  nounPlural = "references",
  ariaLabel,
}: RefPopoverProps) {
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState("");
  const [query, setQuery] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [rows, setRows] = useState<RefRow[]>([]);
  const [total, setTotal] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [page, setPage] = useState(1);
  const listId = useId();

  /*
   * Which request is the current one.
   *
   * A fetcher takes no abort signal - it is a plain promise, so that a caller
   * may hand over anything - which leaves the ignore-stale half of the same
   * job: every request takes a ticket, and a response holding anything but the
   * latest ticket is dropped on the floor. Without it a fast typer whose first
   * query answers after their third would be shown the first one's rows under
   * the third one's text. The ticket is a ref rather than state because it is
   * read inside an async body that must see the value at resolve time.
   */
  const ticket = useRef(0);

  /*
   * The fetcher, held rather than depended on.
   *
   * A caller builds one per chip - `(page, q) => fetchInbound(..., rel)` closes
   * over the relation - and a caller must not have to memoize it for this to
   * work: an unmemoized prop in the loader's dependency list would re-run the
   * loader on every render, which is a request loop, not a bug you find in
   * review. So the identity of the prop is nobody's business and the current
   * value is always the one called.
   */
  const fetcher = useRef(fetchPage);
  useEffect(() => {
    fetcher.current = fetchPage;
  }, [fetchPage]);

  const load = useCallback(
    (nextPage: number, text: string, append: boolean) => {
      ticket.current += 1;
      const mine = ticket.current;
      setPhase("loading");
      setError(null);
      void fetcher.current(nextPage, text).then(
        (result) => {
          if (ticket.current !== mine) {
            return;
          }
          setRows((previous) =>
            append ? [...previous, ...result.rows] : result.rows,
          );
          setTotal(result.total);
          setHasMore(result.hasMore);
          setPage(nextPage);
          setPhase("ready");
        },
        (cause: unknown) => {
          if (ticket.current !== mine) {
            return;
          }
          setError(cause instanceof Error ? cause.message : String(cause));
          setPhase("failed");
        },
      );
    },
    [],
  );

  /*
   * The filter settles before it is sent, so a typed word costs one request
   * rather than one per keystroke: every change restarts the timer, and only
   * the last keystroke of a burst survives it.
   *
   * The load happens inside the timer rather than in a second effect watching
   * the settled value, which is what keeps this an effect that talks to the
   * outside rather than one that feeds itself: a `setState` in an effect body
   * is a cascading render, and this project's lint refuses one.
   */
  useEffect(() => {
    const settled = typed.trim();
    if (!open || settled === query) {
      return;
    }
    const timer = setTimeout(() => {
      setQuery(settled);
      load(1, settled, false);
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [typed, open, query, load]);

  const onOpenChange = (next: boolean) => {
    setOpen(next);
    if (next) {
      // Opening reads: a popover that reopened onto what it read last time
      // would be showing a set that may have changed while it was shut.
      setTyped("");
      setQuery("");
      setPage(1);
      load(1, "", false);
      return;
    }
    // A closed popover holds no stale list and no stale filter, and no answer
    // still in flight may write into one: the next opening starts from the
    // top, which is where a reader expects it.
    ticket.current += 1;
    setTyped("");
    setQuery("");
    setRows([]);
    setPhase("idle");
    setError(null);
    setPage(1);
  };

  return (
    <Popover.Root open={open} onOpenChange={onOpenChange}>
      <Popover.Trigger
        type="button"
        aria-label={ariaLabel ?? `${label}, ${plural(count, noun, nounPlural)}`}
        className={`${CHIP_SHAPE} ${open ? CHIP.open : CHIP.closed}`}
      >
        <span>{label}</span>
        <span className="font-mono">{count}</span>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="start"
          sideOffset={4}
          className={`${MENU_CLASSES} w-80 max-w-[90vw]`}
        >
          <div className="flex flex-col gap-2 p-1">
            <label className="flex flex-col gap-1">
              <span className="text-caption text-slate-600 dark:text-slate-400">
                Filter {nounPlural}
              </span>
              <input
                type="search"
                value={typed}
                autoFocus
                aria-controls={listId}
                onChange={(event) => {
                  setTyped(event.target.value);
                }}
                className={`rounded border border-slate-300 px-2 py-1 text-sm dark:border-slate-700 dark:bg-slate-950 ${FOCUS_RING}`}
              />
            </label>
            {/*
              One line for the state, always in the same place: a reader
              watching for their filter to land does not have to hunt for
              where the answer appeared.
            */}
            <p
              className="text-caption text-slate-600 dark:text-slate-400"
              role={phase === "failed" ? "alert" : undefined}
            >
              {phase === "failed"
                ? `Could not be read: ${error ?? "unknown"}`
                : phase === "loading" && rows.length === 0
                  ? "Loading"
                  : plural(total, noun, nounPlural)}
            </p>
            <ul
              id={listId}
              // Bounded on purpose: pages accumulate as they are asked for, and
              // the box scrolls rather than the popover growing off the screen.
              className="flex max-h-64 flex-col gap-1 overflow-y-auto"
            >
              {rows.map((row) => (
                <li key={row.key} className={row.className}>
                  <Popover.Close asChild>
                    <Link
                      to={row.href}
                      aria-label={`${row.title}, ${row.detail}`}
                      className={`block rounded px-1 py-0.5 hover:bg-slate-100 dark:hover:bg-slate-800 ${FOCUS_RING}`}
                    >
                      <span className="block truncate text-sky-700 underline underline-offset-2 dark:text-sky-400">
                        {row.title}
                      </span>
                      <span className="block truncate text-caption text-slate-600 dark:text-slate-400">
                        {row.detail}
                      </span>
                    </Link>
                  </Popover.Close>
                </li>
              ))}
            </ul>
            {phase === "ready" && rows.length === 0 && (
              <p className="text-caption text-slate-600 dark:text-slate-400">
                Nothing here matches that.
              </p>
            )}
            {hasMore && (
              <button
                type="button"
                disabled={phase === "loading"}
                onClick={() => {
                  load(page + 1, query, true);
                }}
                className={`rounded border border-slate-300 px-2 py-1 text-caption hover:bg-slate-100 disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800 ${FOCUS_RING}`}
              >
                Load more
              </button>
            )}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
