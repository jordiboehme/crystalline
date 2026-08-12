/**
 * The insert-table button, with the one question it always had to ask: how
 * big?
 *
 * The old button inserted a 2x2 and left the rest to hand-typed pipes. This
 * one opens a grid, and the cell the keyboard already sits on IS that 2x2, so
 * the shortest route costs exactly one more keypress than before and every
 * other size costs the arrows it takes to reach it. The grid's top row is the
 * header row, which is why a 2x2 pick means a header and one row to fill.
 *
 * A Popover rather than a DropdownMenu: a menu owns its arrow keys for moving
 * between items, and here the arrows mean "bigger table". A popover brings the
 * dismissal and focus discipline without the navigation, so this component
 * owns its keyboard outright - the reasoning `SuggestInput` already follows.
 *
 * Focus moves with the size rather than being tracked beside it, whichever
 * changed it: Enter is a native activation of the FOCUSED cell, so a
 * highlight that could drift away from the focus - hovered one size, focused
 * another - would insert a size nobody was looking at.
 */

import type { EditorView } from "@codemirror/view";
import { Table } from "lucide-react";
import { Popover } from "radix-ui";
import type { KeyboardEvent, ReactElement } from "react";
import { useRef, useState } from "react";

import { MENU_CLASSES } from "../components/menu";
import { FOCUS_RING, IconButton } from "../components/primitives";
import type { BlockSelection } from "./toolbar";
import { insertBlock, selectToken, tableSkeleton } from "./toolbar";

/** How far the grid goes. Beyond this a table is written, not clicked. */
const COLUMNS = 8;
const ROWS = 6;

/** Where it opens: the size the button used to insert on its own. */
const DEFAULT_SIZE = { columns: 2, rows: 2 } as const;

/** The word the caret arrives on, ready to be typed over. */
const PLACEHOLDER = "Column";

/**
 * A cell's two faces, each a whole class string.
 *
 * Two faces rather than accent utilities layered onto one, for the reason
 * `TOGGLE` in the primitives spells out: Tailwind resolves same-specificity
 * conflicts by emission order, not by the order of names in a class
 * attribute, so a highlight written as an override would silently never
 * appear. Every color is declared exactly once, by exactly one face.
 *
 * The state is carried by the BORDER - accent-600 is 3.74:1 on the light menu
 * surface and 3.32:1 on the wash it encloses, accent-400 is 9.59:1 on the
 * dark surface and 5.09:1 on its own wash, all clear of the 3:1 floor for
 * non-text UI - with the wash as the redundant affordance. The washes
 * themselves are ruling rather than state (accent-100 is 1.13:1 on white,
 * accent-900 1.88:1 on the dark surface, and the unhighlighted cells are
 * fainter still at 1.23:1 and 1.72:1), which is the same trade every divider
 * in this app makes: they draw the grid, the border says which part of it is
 * chosen. The off face reserves the border in `transparent`, so highlighting
 * changes color and nothing moves.
 */
const CELL = {
  off: "border border-transparent bg-slate-200 dark:bg-slate-700",
  on: "border border-accent-600 bg-accent-100 dark:border-accent-400 dark:bg-accent-900",
} as const;

/** The geometry both faces share: a small square, and the app's focus ring. */
const CELL_SHAPE = `h-4 w-4 rounded-xs ${FOCUS_RING}`;

/** Keep a size on the grid however far an arrow key is held down. */
function clamp(value: number, high: number): number {
  return Math.min(high, Math.max(1, value));
}

export function TableSizePicker({
  view,
}: {
  view: EditorView | null;
}): ReactElement {
  const [open, setOpen] = useState(false);
  const [size, setSize] = useState<{ columns: number; rows: number }>(
    DEFAULT_SIZE,
  );

  /*
   * The cells by size, so the keyboard can reach one without a query.
   *
   * A map rather than one ref per cell: the arrows compute a size and then
   * have to move the focus onto whatever element is drawing it, which is a
   * lookup by coordinates. The key is the coordinates themselves.
   */
  const cells = useRef(new Map<string, HTMLButtonElement | null>());
  const key = (columns: number, rows: number) =>
    `${String(columns)}:${String(rows)}`;
  const focusCell = (columns: number, rows: number) => {
    cells.current.get(key(columns, rows))?.focus();
  };

  const resize = (columns: number, rows: number) => {
    const next = { columns: clamp(columns, COLUMNS), rows: clamp(rows, ROWS) };
    setSize(next);
    focusCell(next.columns, next.rows);
  };

  const insert = (columns: number, rows: number) => {
    if (view) {
      const lines = tableSkeleton(columns, rows);
      const select: BlockSelection | null = selectToken(lines, PLACEHOLDER);
      insertBlock(view, lines, select);
    }
    setOpen(false);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step: Record<string, [number, number]> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, -1],
      ArrowDown: [0, 1],
    };
    const move = step[event.key];
    if (!move) {
      return;
    }
    // The buffer scrolls on arrow keys and so does the page; neither is what
    // an open grid means by them.
    event.preventDefault();
    resize(size.columns + move[0], size.rows + move[1]);
  };

  return (
    <Popover.Root
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) {
          // Every opening starts where the button used to insert, rather than
          // on whatever the last table happened to need.
          setSize(DEFAULT_SIZE);
        }
      }}
    >
      <Popover.Trigger asChild>
        <IconButton
          label="Insert table"
          icon={Table}
          disabled={view === null}
        />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="start"
          sideOffset={6}
          className={`${MENU_CLASSES} min-w-0`}
          onKeyDown={onKeyDown}
          // Radix focuses the content itself on open, which would leave the
          // arrows with nothing to move and Enter with nothing to press. The
          // default cell takes it instead, so the whole control is one Enter
          // away for a person who never touches the pointer.
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            focusCell(DEFAULT_SIZE.columns, DEFAULT_SIZE.rows);
          }}
          // And on the way out the buffer takes it back, having inserted or
          // having been dismissed - the discipline the heading menu in this
          // bar already follows.
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            view?.focus();
          }}
        >
          <div
            role="group"
            aria-label="Table size"
            className="flex flex-col gap-1 p-1"
          >
            {Array.from({ length: ROWS }, (_, index) => index + 1).map(
              (rows) => (
                <div key={rows} className="flex gap-1">
                  {Array.from({ length: COLUMNS }, (_, index) => index + 1).map(
                    (columns) => (
                      <button
                        key={columns}
                        type="button"
                        // The size in words, because "3 x 4" read aloud is not
                        // a size - it is three letters and two numbers.
                        aria-label={`${String(columns)} columns by ${String(rows)} rows`}
                        ref={(element) => {
                          cells.current.set(key(columns, rows), element);
                        }}
                        className={`${CELL_SHAPE} ${
                          columns <= size.columns && rows <= size.rows
                            ? CELL.on
                            : CELL.off
                        }`}
                        onMouseEnter={() => {
                          // The pointer moves the focus with the highlight
                          // like the arrows do. Highlighting alone would let
                          // the two drift apart - hover 5x4, press Enter, and
                          // the cell that is still focused inserts the size
                          // nobody is looking at.
                          resize(columns, rows);
                        }}
                        onClick={() => {
                          insert(columns, rows);
                        }}
                      />
                    ),
                  )}
                </div>
              ),
            )}
          </div>
          {/*
            The caption is for the eye only: every cell says its own size to a
            screen reader, and hearing it twice would be noise. The app's own
            muted pair rather than a step lighter, which is what the measurement
            settles: slate-600 is 7.58:1 on the light menu surface and slate-400
            is 6.96:1 on the dark one, where slate-500 would read 4.76:1 light
            but only 3.75:1 dark - under the floor for text this size.
          */}
          <p
            aria-hidden="true"
            className="pt-1 text-center text-caption text-slate-600 dark:text-slate-400"
          >
            {size.columns} x {size.rows}
          </p>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
