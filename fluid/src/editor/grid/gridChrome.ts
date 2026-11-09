/**
 * The chrome around a drawn table, in plain DOM inside the grid's root:
 *
 * - **Hover "+"**: near the border between two header cells a button named
 *   "Insert column here" appears on that border; near the top of a data row's
 *   first cell, one named "Insert row here" appears on the left edge. Both
 *   are mouse affordances; their keyboard twins are in the menu below.
 * - **Cell actions**: a `role="menu"` of the table verbs, opened by right
 *   click, Shift+F10 or the context-menu key on a cell (grid navigation or
 *   the open cell editor), and by a small "Cell actions" button drawn on the
 *   focused or open cell only, never on hover alone. It stands on the cell's
 *   top-right corner, half outside the cell, so its inside half lies in the
 *   cell's right padding: it never covers text and never takes a click on
 *   another cell. The menu is `position: fixed` from its anchor, so the
 *   editor's scroller never clips it, and it flips above the anchor when
 *   there is no room below.
 *
 * Every action is one structural verb of `tableVerbs.ts` run at the cell's
 * document position with `{ focus: false }` (R15: the main view never takes
 * the focus), between two closed undo groups (R14), and then opens the cell
 * editor in the cell the action moved or made. A page that is not writable
 * (R17) gets none of it: no buttons, no menu, the browser's own context menu.
 */

import type { ChangeDesc } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

import {
  tableAddColumnAfter,
  tableAddColumnBefore,
  tableAddRowAbove,
  tableAddRowBelow,
  tableAlignColumn,
  tableDeleteColumn,
  tableDeleteRow,
  tableMoveColumn,
  tableMoveRow,
} from "../tableVerbs";
import { isMenuKey } from "../../platform";
import { activeCell, noteUndoLanding } from "./cellEditor";
import { closeUndoGroup } from "./gridKeys";
import {
  cellElement,
  hostOpenCell,
  placeOf,
  rove,
  tableOf,
} from "./gridWidget";
import {
  closeCell,
  gridWritable,
  isDestroyed,
  openCellField,
  openCellRange,
  parseThrough,
  setOpenCell,
} from "./openCell";
import type { GridTable } from "./structure";
import { cellAt, gridTableAt } from "./structure";

/** How close to a border (in pixels) the pointer has to be for a "+". */
const EDGE = 8;

type Align = "left" | "center" | "right";

interface Action {
  label: string;
  /** Radio items carry the alignment they set. */
  align?: Align;
  /** Whether the verb can apply to the cell at `row` and `column`. */
  applies(table: GridTable, row: number, column: number): boolean;
  run(main: EditorView, at: number, row: number, column: number): boolean;
  /** The cell to open afterwards, or null to leave the cells as they are. */
  land(row: number, column: number): [number, number] | null;
  /**
   * Adds a column: a landing in the header is the new header, whose
   * placeholder text ("Column", R5) opens selected so typing replaces it.
   */
  addsColumn?: boolean;
}

const always = (): boolean => true;

const ACTIONS: Action[] = [
  {
    label: "Insert row above",
    applies: (_t, row) => row >= 1,
    run: (main, at) => tableAddRowAbove(main, at, { focus: false }),
    land: (row, column) => [row, column],
  },
  {
    label: "Insert row below",
    applies: always,
    run: (main, at) => tableAddRowBelow(main, at, { focus: false }),
    land: (row, column) => [row + 1, column],
  },
  {
    label: "Move row up",
    applies: (_t, row) => row >= 2,
    run: (main, at) => tableMoveRow(main, -1, at, { focus: false }),
    land: (row, column) => [row - 1, column],
  },
  {
    label: "Move row down",
    applies: (table, row) => row >= 1 && row < table.rows.length - 1,
    run: (main, at) => tableMoveRow(main, 1, at, { focus: false }),
    land: (row, column) => [row + 1, column],
  },
  {
    label: "Delete row",
    applies: (_t, row) => row >= 1,
    run: (main, at) => tableDeleteRow(main, at, { focus: false }),
    land: () => null,
  },
  {
    label: "Insert column left",
    applies: always,
    run: (main, at) => tableAddColumnBefore(main, at, { focus: false }),
    land: (row, column) => [row, column],
    addsColumn: true,
  },
  {
    label: "Insert column right",
    applies: always,
    run: (main, at) => tableAddColumnAfter(main, at, { focus: false }),
    land: (row, column) => [row, column + 1],
    addsColumn: true,
  },
  {
    label: "Move column left",
    applies: (_t, _row, column) => column >= 1,
    run: (main, at) => tableMoveColumn(main, -1, at, { focus: false }),
    land: (row, column) => [row, column - 1],
  },
  {
    label: "Move column right",
    applies: (table, row, column) =>
      column < (table.rows[row]?.length ?? 1) - 1,
    run: (main, at) => tableMoveColumn(main, 1, at, { focus: false }),
    land: (row, column) => [row, column + 1],
  },
  {
    label: "Delete column",
    applies: (table, row) => (table.rows[row]?.length ?? 1) > 1,
    run: (main, at) => tableDeleteColumn(main, at, { focus: false }),
    land: () => null,
  },
  ...(["left", "center", "right"] as const).map((align): Action => ({
    label: `Align ${align}`,
    align,
    applies: always,
    run: (main, at) => tableAlignColumn(main, align, at, { focus: false }),
    land: () => null,
  })),
];

const chromes = new WeakMap<HTMLElement, () => void>();

/**
 * Where a menu opens, in viewport coordinates: below `y` from `x`, or above
 * `top` (the anchor's top edge, `y` when absent) with no room below.
 */
export interface MenuAnchor {
  x: number;
  y: number;
  top?: number;
}

/** The small Cell actions button's size; half of it stands outside. */
const CELL_BUTTON = 16;

/** Each grid root's menu opener, for the floating bar's "More table actions". */
const openers = new WeakMap<
  HTMLElement,
  (row: number, column: number, anchor: MenuAnchor, back?: HTMLElement) => void
>();

/** Each grid root's "the table changed under the menu" hook. */
const changes = new WeakMap<HTMLElement, () => void>();

/**
 * Where each main view's open Cell actions menu points, as a document
 * position mapped through every change (`mapChromeMenus`), so the focus can
 * find its cell again when a rebuild takes the menu away.
 */
const menuMarks = new WeakMap<EditorView, Set<{ pos: number }>>();

function marksOf(main: EditorView): Set<{ pos: number }> {
  let marks = menuMarks.get(main);
  if (!marks) {
    marks = new Set();
    menuMarks.set(main, marks);
  }
  return marks;
}

/** Maps every open menu's cell position of `main` through `desc`. */
export function mapChromeMenus(main: EditorView, desc: ChangeDesc): void {
  for (const mark of menuMarks.get(main) ?? []) {
    mark.pos = desc.mapPos(mark.pos);
  }
}

/** Tears down a grid root's chrome (the widget's `destroy` calls it). */
export function disposeChrome(root: HTMLElement): void {
  chromes.get(root)?.();
  chromes.delete(root);
  changes.delete(root);
}

/**
 * Tells a grid root's chrome that a cell text or an alignment of its table
 * changed in place (a peer or an agent, the shape kept). An open menu closes
 * then, focus back where it came from: its row and column numbers may now
 * name somebody else's row.
 */
export function tableChangedUnderChrome(root: HTMLElement): void {
  changes.get(root)?.();
}

/**
 * Puts the focus back in the table a rebuild took a focused menu out of:
 * the open cell when there is one, else the cell the menu was opened on, and
 * main only as a last resort, so the focus never stays on the page body.
 */
function refocusAfterRebuild(main: EditorView, pos: number): void {
  if (isDestroyed(main)) return;
  const active = document.activeElement;
  if (active !== null && active !== document.body) return;
  if (main.state.field(openCellField, false) && hostOpenCell(main, "now")) {
    return;
  }
  const table = gridTableAt(main.state, pos);
  const root = table ? rootAt(main, table.from) : null;
  const cell = table ? cellAt(table, pos) : null;
  const el = root && cell ? cellElement(root, cell.row, cell.column) : null;
  if (el) {
    rove(el);
    el.focus();
    return;
  }
  main.focus();
}

function makeButton(label: string, text: string, cls: string): HTMLElement {
  const el = document.createElement("button");
  el.type = "button";
  el.className = `cm-grid-chrome ${cls}`;
  el.setAttribute("aria-label", label);
  el.textContent = text;
  el.hidden = true;
  // A press never moves the focus or the selection: the click acts on the
  // cell the button stands at.
  el.addEventListener("mousedown", (event) => {
    event.preventDefault();
  });
  return el;
}

/** The grid root holding the table that starts at `from`, if drawn. */
function rootAt(main: EditorView, from: number): HTMLElement | null {
  for (const root of main.contentDOM.querySelectorAll<HTMLElement>(
    ".cm-grid",
  )) {
    if (tableOf(main, root)?.from === from) return root;
  }
  return null;
}

/**
 * Runs `run` on the cell at `row`/`column` between two closed undo groups,
 * then opens the cell `land` names. `restoreFocus` is where the focus goes
 * when no cell opens.
 */
function performAt(
  main: EditorView,
  root: HTMLElement,
  row: number,
  column: number,
  run: (at: number) => boolean,
  land: [number, number] | null,
  restoreFocus: HTMLElement | null,
  reshapes = true,
  addsColumn = false,
): void {
  const table = tableOf(main, root);
  const cell = table?.rows[row]?.[column];
  if (!table || !cell || !gridWritable(main.state)) return;
  const open = main.state.field(openCellField, false);
  const before = main.state.doc;
  // A change of shape can take the open cell's pipes with it, and a cell
  // that goes that way hands the focus to the main view. Close it first,
  // here, with the focus nowhere; the landing (or an undo) opens one again.
  if (open && reshapes) closeCell(main, "user", "none");
  closeUndoGroup(main, "before");
  if (!run(cell.content.from)) {
    // Nothing changed: the cell closed above opens again where it was, or
    // the focus goes back where it came from.
    if (open && reshapes) {
      main.dispatch({ effects: setOpenCell.of({ ...open, focused: true }) });
      if (hostOpenCell(main, "now")) return;
    }
    if (restoreFocus?.isConnected) restoreFocus.focus();
    return;
  }
  closeUndoGroup(main, "after");
  const after = main.state.doc;
  // The verbs change only this table, so its end moves by the length change.
  parseThrough(main, table.to + after.length - before.length);
  if (open) {
    const left = { anchor: open.anchor, head: open.head, focused: true };
    noteUndoLanding(main, {
      before,
      after,
      restore: () => {
        main.dispatch({ effects: setOpenCell.of(left) });
        hostOpenCell(main, "now");
      },
    });
  }
  const grown = gridTableAt(main.state, table.from);
  const target = land ? grown?.rows[land[0]]?.[land[1]] : undefined;
  if (target) {
    // A new header opens with its placeholder selected (finding 10).
    const all = addsColumn && land?.[0] === 0;
    main.dispatch({
      effects: setOpenCell.of({
        anchor: all ? target.content.from : target.content.to,
        head: target.content.to,
        focused: true,
      }),
    });
    hostOpenCell(main, "now");
    return;
  }
  // No new cell: focus goes back where it was, or to the nearest cell.
  if (restoreFocus?.isConnected) {
    restoreFocus.focus();
    return;
  }
  const newRoot = grown ? rootAt(main, grown.from) : null;
  if (!grown || !newRoot) return;
  const r = Math.min(row, grown.rows.length - 1);
  const c = Math.min(column, (grown.rows[r]?.length ?? 1) - 1);
  const el = cellElement(newRoot, r, c);
  if (el) {
    rove(el);
    el.focus();
  }
}

/** Draws the hover buttons and the cell menu for one grid root. */
export function mountChrome(root: HTMLElement, main: EditorView): void {
  const columnPlus = makeButton("Insert column here", "+", "cm-grid-plus");
  const rowPlus = makeButton("Insert row here", "+", "cm-grid-plus");
  const cellButton = makeButton("Cell actions", "⋯", "cm-grid-cell-menu");
  cellButton.setAttribute("aria-haspopup", "menu");
  cellButton.setAttribute("aria-expanded", "false");
  root.append(columnPlus, rowPlus, cellButton);

  /** The border each "+" stands on: the column or row it inserts before. */
  let columnBorder = 0;
  let rowBorder = 0;
  /** The cell the small button stands on. */
  let buttonCell: [number, number] | null = null;
  /** The focused or open cell, which the small button belongs to. */
  let focusedCell: HTMLElement | null = null;
  let menu: HTMLElement | null = null;
  let menuCell: [number, number] | null = null;
  /** The open menu's cell position, mapped through every change. */
  let menuMark: { pos: number } | null = null;
  /** Whether the focus is in the open menu (set by focusin, not read late). */
  let menuFocused = false;
  /** The open menu's anchor, relative to the grid root's box. */
  let menuAnchor: { dx: number; dy: number; above: number } | null = null;
  let returnTo: HTMLElement | null = null;

  const originOf = (): { left: number; top: number } => {
    const rect = root.getBoundingClientRect();
    return { left: rect.left, top: rect.top };
  };

  const hideCellButton = (): void => {
    cellButton.hidden = true;
    buttonCell = null;
  };

  /**
   * Draws the small button on the focused or open cell: centred on the
   * cell's top-right corner, so its inside half lies in the cell's right
   * padding. While a "+" is drawn it steps aside (on the header the two
   * would share a corner).
   */
  const drawCellButton = (): void => {
    const el = focusedCell;
    const plus = !columnPlus.hidden || !rowPlus.hidden;
    if (!el?.isConnected || !root.contains(el) || plus) {
      hideCellButton();
      return;
    }
    if (!gridWritable(main.state)) {
      hideCellButton();
      return;
    }
    buttonCell = placeOf(el);
    const origin = originOf();
    const rect = el.getBoundingClientRect();
    cellButton.style.left = `${String(rect.right - origin.left - CELL_BUTTON / 2)}px`;
    cellButton.style.top = `${String(rect.top - origin.top - CELL_BUTTON / 2)}px`;
    cellButton.hidden = false;
  };

  const hidePlus = (): void => {
    const shown = !columnPlus.hidden || !rowPlus.hidden;
    columnPlus.hidden = true;
    rowPlus.hidden = true;
    if (shown && focusedCell) drawCellButton();
  };

  // A cell that grows while it is typed in moves its corner.
  const resized =
    typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver(() => {
          if (!cellButton.hidden) drawCellButton();
        });
  const table = root.querySelector("table");
  if (resized && table) resized.observe(table);

  const closeMenu = (refocus: boolean): void => {
    document.removeEventListener("mousedown", outside, true);
    document.removeEventListener("scroll", follow, true);
    const open = menu;
    menu = null;
    menuFocused = false;
    menuAnchor = null;
    if (menuMark) menuMarks.get(main)?.delete(menuMark);
    menuMark = null;
    cellButton.setAttribute("aria-expanded", "false");
    if (!open) return;
    open.remove();
    const back = returnTo;
    returnTo = null;
    if (!refocus) return;
    if (back?.isConnected) back.focus();
    else if (menuCell) {
      const table = tableOf(main, root);
      const cell = table ? cellElement(root, ...menuCell) : null;
      cell?.focus();
    }
  };

  /** Places the open menu from its anchor, below it or flipped above. */
  const placeMenu = (): void => {
    if (!menu || !menuAnchor) return;
    const origin = originOf();
    const x = origin.left + menuAnchor.dx;
    const below = origin.top + menuAnchor.dy;
    const above = origin.top + menuAnchor.above;
    const height = menu.offsetHeight;
    const width = menu.offsetWidth;
    const room = window.innerHeight;
    let top = below;
    if (below + height > room && above - height >= 0) top = above - height;
    else if (below + height > room) top = Math.max(0, room - height);
    const left = Math.max(0, Math.min(x, window.innerWidth - width));
    menu.style.left = `${String(left)}px`;
    menu.style.top = `${String(top)}px`;
  };

  /** A scroll anywhere moves the table under a fixed menu: follow it. */
  function follow(): void {
    placeMenu();
  }

  function outside(event: Event): void {
    if (menu && event.target instanceof Node && menu.contains(event.target))
      return;
    if (event.target === cellButton) return; // its click toggles
    closeMenu(false);
  }

  const perform = (
    row: number,
    column: number,
    run: (at: number) => boolean,
    land: [number, number] | null,
    restoreFocus: HTMLElement | null,
    reshapes = true,
    addsColumn = false,
  ): void => {
    performAt(
      main,
      root,
      row,
      column,
      run,
      land,
      restoreFocus,
      reshapes,
      addsColumn,
    );
  };

  const openMenu = (
    row: number,
    column: number,
    anchor: MenuAnchor,
    back?: HTMLElement,
  ): void => {
    if (!gridWritable(main.state)) return;
    const table = tableOf(main, root);
    if (!table?.rows[row]?.[column]) return;
    const active = document.activeElement;
    if (back) returnTo = back;
    else if (!menu) {
      returnTo =
        active instanceof HTMLElement &&
        active !== document.body &&
        root.contains(active)
          ? active
          : cellElement(root, row, column);
    }
    closeMenu(false);
    menuCell = [row, column];
    const cell = table.rows[row]?.[column];
    if (cell) {
      menuMark = { pos: cell.content.from };
      marksOf(main).add(menuMark);
    }
    const el = document.createElement("div");
    el.setAttribute("role", "menu");
    el.setAttribute("aria-label", "Cell actions");
    el.className = "cm-grid-chrome cm-grid-menu";
    // Fixed, so the editor's scroller cannot clip it (finding 6).
    el.style.position = "fixed";
    const origin = originOf();
    menuAnchor = {
      dx: anchor.x - origin.left,
      dy: anchor.y - origin.top,
      above: (anchor.top ?? anchor.y) - origin.top,
    };
    el.addEventListener("focusin", () => {
      menuFocused = true;
    });
    el.addEventListener("focusout", (event) => {
      const next = event.relatedTarget;
      if (!(next instanceof Node && el.contains(next))) menuFocused = false;
    });
    for (const action of ACTIONS) {
      const entry = document.createElement("div");
      entry.className = "cm-grid-menu-item";
      entry.tabIndex = -1;
      entry.textContent = action.label;
      if (action.align) {
        entry.setAttribute("role", "menuitemradio");
        entry.setAttribute(
          "aria-checked",
          String(table.aligns[column] === action.align),
        );
      } else entry.setAttribute("role", "menuitem");
      const ok = action.applies(table, row, column);
      if (!ok) entry.setAttribute("aria-disabled", "true");
      entry.addEventListener("click", () => {
        if (!ok) return;
        const back = returnTo;
        closeMenu(false);
        perform(
          row,
          column,
          (at) => action.run(main, at, row, column),
          action.land(row, column),
          back,
          action.align === undefined,
          action.addsColumn ?? false,
        );
      });
      el.appendChild(entry);
    }
    el.addEventListener("keydown", (event) => {
      // The menu key on its own menu opens no second one and runs no item: on a
      // Mac it is Ctrl+Return, which the Enter case below would take (R111, I3).
      if (isMenuKey(event)) {
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      const entries = [
        ...el.querySelectorAll<HTMLElement>('[role^="menuitem"]'),
      ];
      const index = entries.indexOf(event.target as HTMLElement);
      const go = (to: number) => {
        entries[(to + entries.length) % entries.length]?.focus();
      };
      switch (event.key) {
        case "ArrowDown":
          go(index + 1);
          break;
        case "ArrowUp":
          go(index < 0 ? -1 : index - 1);
          break;
        case "Home":
          go(0);
          break;
        case "End":
          go(-1);
          break;
        case "Enter":
        case " ":
          (event.target as HTMLElement).click();
          break;
        case "Escape":
          closeMenu(true);
          break;
        case "Tab":
          closeMenu(true);
          break;
        default:
          return;
      }
      event.preventDefault();
      event.stopPropagation();
    });
    root.appendChild(el);
    menu = el;
    placeMenu();
    cellButton.setAttribute("aria-expanded", "true");
    document.addEventListener("mousedown", outside, true);
    document.addEventListener("scroll", follow, {
      capture: true,
      passive: true,
    });
    el.querySelector<HTMLElement>('[role^="menuitem"]')?.focus({
      preventScroll: true,
    });
  };

  const cellOf = (target: EventTarget | null): HTMLElement | null => {
    if (!(target instanceof Element)) return null;
    const el = target.closest<HTMLElement>("[aria-rowindex][aria-colindex]");
    return el && root.contains(el) ? el : null;
  };

  const placeButton = (
    button: HTMLElement,
    x: number,
    y: number,
    half: number,
  ): void => {
    const origin = originOf();
    button.style.left = `${String(x - origin.left - half)}px`;
    button.style.top = `${String(y - origin.top - half)}px`;
    button.hidden = false;
    hideCellButton();
  };

  root.addEventListener("mousemove", (event) => {
    if (!gridWritable(main.state)) {
      hidePlus();
      return;
    }
    const target = event.target as Element;
    if (target.closest(".cm-grid-chrome")) return; // moving onto a button
    const el = cellOf(target);
    if (!el) return;
    const table = tableOf(main, root);
    if (!table) return;
    const [row, column] = placeOf(el);
    const rect = el.getBoundingClientRect();
    hidePlus();
    if (row === 0) {
      const width = table.rows[0]?.length ?? 0;
      let border = 0;
      let x = rect.left;
      if (column > 0 && event.clientX - rect.left <= EDGE) border = column;
      else if (column + 1 < width && rect.right - event.clientX <= EDGE) {
        border = column + 1;
        x = rect.right;
      }
      if (border > 0) {
        columnBorder = border;
        placeButton(columnPlus, x, rect.top, 9);
      }
    } else if (column === 0) {
      const last = table.rows.length - 1;
      let border = 0;
      let y = rect.top;
      if (event.clientY - rect.top <= EDGE) border = row;
      else if (row < last && rect.bottom - event.clientY <= EDGE) {
        border = row + 1;
        y = rect.bottom;
      }
      if (border > 0) {
        rowBorder = border;
        placeButton(rowPlus, rect.left - 9, y, 9);
      }
    }
  });

  root.addEventListener("mouseleave", () => {
    hidePlus();
  });

  // The focused cell (grid navigation) or the open one (the focus is in its
  // editor, inside the cell): the only cells the small button is drawn on.
  root.addEventListener("focusin", (event) => {
    const el = cellOf(event.target);
    if (!el) return;
    focusedCell = el;
    drawCellButton();
  });

  root.addEventListener("focusout", (event) => {
    const next = event.relatedTarget;
    if (next instanceof Node && root.contains(next)) return;
    if (menu) return;
    focusedCell = null;
    hideCellButton();
  });

  root.addEventListener("contextmenu", (event) => {
    if (!gridWritable(main.state)) return;
    const el = cellOf(event.target);
    if (!el) return;
    event.preventDefault();
    const [row, column] = placeOf(el);
    openMenu(row, column, { x: event.clientX, y: event.clientY });
  });

  root.addEventListener("keydown", (event) => {
    // A held key opens the menu once (I3).
    if (event.defaultPrevented || event.isComposing || event.repeat) return;
    // Shift+F10, the menu key, and on a Mac Ctrl+Return (R111).
    if (!isMenuKey(event) || !gridWritable(main.state)) return;
    const el = cellOf(event.target);
    if (!el) return;
    event.preventDefault();
    const [row, column] = placeOf(el);
    const rect = el.getBoundingClientRect();
    openMenu(row, column, { x: rect.left, y: rect.bottom, top: rect.top });
  });

  cellButton.addEventListener("click", () => {
    if (menu) {
      closeMenu(true);
      return;
    }
    if (buttonCell) {
      const rect = cellButton.getBoundingClientRect();
      openMenu(buttonCell[0], buttonCell[1], {
        x: rect.left,
        y: rect.bottom,
        top: rect.top,
      });
    }
  });

  columnPlus.addEventListener("click", () => {
    const column = columnBorder;
    hidePlus();
    perform(
      0,
      column,
      (at) => tableAddColumnBefore(main, at, { focus: false }),
      [0, column],
      null,
      true,
      true,
    );
  });

  rowPlus.addEventListener("click", () => {
    const row = rowBorder;
    hidePlus();
    perform(
      row,
      0,
      (at) => tableAddRowAbove(main, at, { focus: false }),
      [row, 0],
      null,
    );
  });

  openers.set(root, openMenu);
  changes.set(root, () => {
    if (menu) closeMenu(true);
  });
  chromes.set(root, () => {
    openers.delete(root);
    resized?.disconnect();
    // A rebuild (a shape change by anyone) takes a focused menu with it:
    // the focus goes back into the table instead of falling to the body.
    const mark = menuMark;
    const hadFocus = menu !== null && menuFocused;
    closeMenu(false);
    if (hadFocus && mark) {
      queueMicrotask(() => {
        refocusAfterRebuild(main, mark.pos);
      });
    }
  });
}

/** Where `main`'s open cell is drawn: its grid root, row and column. */
function openPlace(
  main: EditorView,
): { root: HTMLElement; table: GridTable; row: number; column: number } | null {
  const cell = openCellRange(main.state);
  if (!cell) return null;
  const table = gridTableAt(main.state, cell.span.from);
  const root = table ? rootAt(main, table.from) : null;
  if (!table || !root) return null;
  return { root, table, row: cell.row, column: cell.column };
}

/** The Cell actions entry called `label`. */
function actionNamed(label: string): Action | null {
  return ACTIONS.find((action) => action.label === label) ?? null;
}

/**
 * Whether the Cell actions entry called `label` can apply to `main`'s open
 * cell (Delete row is off on the header, and so on).
 */
export function cellActionApplies(main: EditorView, label: string): boolean {
  const place = openPlace(main);
  const action = actionNamed(label);
  if (!place || !action || !gridWritable(main.state)) return false;
  return action.applies(place.table, place.row, place.column);
}

/**
 * Runs the Cell actions entry called `label` on `main`'s open cell, exactly
 * as the menu runs it (R14 undo groups, R15 no focus to main, the landing
 * cell opened). When no cell opens afterwards the focus goes back to the
 * open cell's editor if it is still there.
 */
export function runCellAction(main: EditorView, label: string): boolean {
  const place = openPlace(main);
  const action = actionNamed(label);
  if (!place || !action || !cellActionApplies(main, label)) return false;
  const { root, row, column } = place;
  performAt(
    main,
    root,
    row,
    column,
    (at) => action.run(main, at, row, column),
    action.land(row, column),
    activeCell(main)?.view.contentDOM ?? null,
    action.align === undefined,
    action.addsColumn ?? false,
  );
  return true;
}

/**
 * Opens the Cell actions menu for `main`'s open cell at `anchor` (viewport
 * coordinates); closing it hands the focus back to the cell's editor.
 */
export function openCellActions(main: EditorView, anchor: MenuAnchor): boolean {
  const place = openPlace(main);
  const open = place ? openers.get(place.root) : undefined;
  if (!place || !open) return false;
  open(
    place.row,
    place.column,
    anchor,
    activeCell(main)?.view.contentDOM ?? undefined,
  );
  return true;
}
