/**
 * The chrome around a drawn table, in plain DOM inside the grid's root:
 *
 * - **Hover "+"**: near the border between two header cells a button named
 *   "Insert column here" appears on that border; near the top of a data row's
 *   first cell, one named "Insert row here" appears on the left edge. Both
 *   are mouse affordances; their keyboard twins are in the menu below.
 * - **Cell actions**: a `role="menu"` of the table verbs, opened by right
 *   click, Shift+F10 or the context-menu key on a cell (grid navigation or
 *   the open cell editor), and by a small "Cell actions" button shown on the
 *   focused or hovered cell.
 *
 * Every action is one structural verb of `tableVerbs.ts` run at the cell's
 * document position with `{ focus: false }` (R15: the main view never takes
 * the focus), between two closed undo groups (R14), and then opens the cell
 * editor in the cell the action moved or made. A page that is not writable
 * (R17) gets none of it: no buttons, no menu, the browser's own context menu.
 */

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
import { noteUndoLanding } from "./cellEditor";
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
  openCellField,
  setOpenCell,
} from "./openCell";
import type { GridTable } from "./structure";
import { gridTableAt } from "./structure";

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
  },
  {
    label: "Insert column right",
    applies: always,
    run: (main, at) => tableAddColumnAfter(main, at, { focus: false }),
    land: (row, column) => [row, column + 1],
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

/** Tears down a grid root's chrome (the widget's `destroy` calls it). */
export function disposeChrome(root: HTMLElement): void {
  chromes.get(root)?.();
  chromes.delete(root);
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
  let menu: HTMLElement | null = null;
  let menuCell: [number, number] | null = null;
  let returnTo: HTMLElement | null = null;

  const originOf = (): { left: number; top: number } => {
    const rect = root.getBoundingClientRect();
    return { left: rect.left, top: rect.top };
  };

  const hidePlus = (): void => {
    columnPlus.hidden = true;
    rowPlus.hidden = true;
  };

  const hideCellButton = (): void => {
    cellButton.hidden = true;
    buttonCell = null;
  };

  const showCellButton = (el: HTMLElement): void => {
    if (!gridWritable(main.state)) {
      hideCellButton();
      return;
    }
    buttonCell = placeOf(el);
    const origin = originOf();
    const rect = el.getBoundingClientRect();
    cellButton.style.left = `${String(rect.right - origin.left - 22)}px`;
    cellButton.style.top = `${String(rect.top - origin.top + 2)}px`;
    cellButton.hidden = false;
  };

  const closeMenu = (refocus: boolean): void => {
    document.removeEventListener("mousedown", outside, true);
    const open = menu;
    menu = null;
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

  function outside(event: Event): void {
    if (menu && event.target instanceof Node && menu.contains(event.target))
      return;
    if (event.target === cellButton) return; // its click toggles
    closeMenu(false);
  }

  /**
   * Runs `run` on the cell at `row`/`column` between two closed undo groups,
   * then opens the cell `land` names. `restoreFocus` is where the focus goes
   * when no cell opens.
   */
  const perform = (
    row: number,
    column: number,
    run: (at: number) => boolean,
    land: [number, number] | null,
    restoreFocus: HTMLElement | null,
    reshapes = true,
  ): void => {
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
    if (!run(cell.content.from)) return;
    closeUndoGroup(main, "after");
    const after = main.state.doc;
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
      main.dispatch({
        effects: setOpenCell.of({
          anchor: target.content.to,
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
  };

  const openMenu = (
    row: number,
    column: number,
    anchor: { x: number; y: number },
  ): void => {
    if (!gridWritable(main.state)) return;
    const table = tableOf(main, root);
    if (!table?.rows[row]?.[column]) return;
    const active = document.activeElement;
    if (!menu) {
      returnTo =
        active instanceof HTMLElement &&
        active !== document.body &&
        root.contains(active)
          ? active
          : cellElement(root, row, column);
    }
    closeMenu(false);
    menuCell = [row, column];
    const el = document.createElement("div");
    el.setAttribute("role", "menu");
    el.setAttribute("aria-label", "Cell actions");
    el.className = "cm-grid-chrome cm-grid-menu";
    const origin = originOf();
    el.style.left = `${String(anchor.x - origin.left)}px`;
    el.style.top = `${String(anchor.y - origin.top)}px`;
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
        );
      });
      el.appendChild(entry);
    }
    el.addEventListener("keydown", (event) => {
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
    cellButton.setAttribute("aria-expanded", "true");
    document.addEventListener("mousedown", outside, true);
    el.querySelector<HTMLElement>('[role^="menuitem"]')?.focus();
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

  root.addEventListener("mouseover", (event) => {
    const el = cellOf(event.target);
    if (el) showCellButton(el);
  });

  root.addEventListener("mouseleave", () => {
    hidePlus();
    if (menu) return;
    if (!root.contains(document.activeElement)) hideCellButton();
  });

  root.addEventListener("focusin", (event) => {
    const el = cellOf(event.target);
    if (el) showCellButton(el);
  });

  root.addEventListener("focusout", (event) => {
    const next = event.relatedTarget;
    if (next instanceof Node && root.contains(next)) return;
    if (menu) return;
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
    if (event.defaultPrevented || event.isComposing) return;
    const menuKey =
      event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey);
    if (!menuKey || !gridWritable(main.state)) return;
    const el = cellOf(event.target);
    if (!el) return;
    event.preventDefault();
    const [row, column] = placeOf(el);
    const rect = el.getBoundingClientRect();
    openMenu(row, column, { x: rect.left, y: rect.bottom });
  });

  cellButton.addEventListener("click", () => {
    if (menu) {
      closeMenu(true);
      return;
    }
    if (buttonCell) {
      const rect = cellButton.getBoundingClientRect();
      openMenu(buttonCell[0], buttonCell[1], { x: rect.left, y: rect.bottom });
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

  chromes.set(root, () => {
    closeMenu(false);
  });
}
