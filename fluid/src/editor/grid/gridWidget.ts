/**
 * The table grid: every table `gridDraws` accepts is replaced, whole lines,
 * by a `role="grid"` block widget whose cells draw their Markdown statically
 * (`cellRender.ts`) and host the one cell editor (`cellEditor.ts`) in the
 * cell the open-cell intent names.
 *
 * Three rules keep the hosted editor alive through the document changing
 * under it:
 *
 * - **Patch in place.** While a table keeps its shape, `updateDOM` redraws
 *   only the cells whose text changed and never touches the cell hosting the
 *   editor. While that editor is composing, the patch waits for
 *   `compositionend`.
 * - **Remount from intent.** Whenever the widget is built (`toDOM`) and
 *   `openCellField` points into its table, it mounts the cell editor in the
 *   matching cell, with the caret from the field and focus when the field
 *   says the cell had it. That covers a shape change by anyone and a table
 *   coming back into view after virtualisation.
 * - **Destroy without closing.** `destroy` tears the hosted editor down and
 *   leaves the intent, which is what the remount reads.
 *
 * The main view a grid belongs to is kept in a WeakMap on the widget's root:
 * `EditorView.findFromDOM` would find the nested cell view first.
 */

import type { Extension, Range } from "@codemirror/state";
import { EditorState, StateField } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";

import type { WikilinkResolver } from "../../wikilinks";
import { elementEditCell } from "../elementEdit";
import { floatingCell } from "../floatingContext";
import { footnoteRefsIn, footnoteVersion } from "../footnotes";
import { hidesIdLines } from "../hiddenIdLines";
import { formatCell } from "../marks/formatting";
import { parseAdvanced } from "../parseProgress";
import { wikilinkResolverFacet } from "../wikilinkChips";
import type { CellEditor } from "./cellEditor";
import { activeCell, mountCellEditor } from "./cellEditor";
import { renderCellContent } from "./cellRender";
import {
  disposeChrome,
  mapChromeMenus,
  mountChrome,
  tableChangedUnderChrome,
} from "./gridChrome";
import { cellKeymap, gridNavKeys } from "./gridKeys";
import { cellPaste } from "./gridPaste";
import {
  gridWritable,
  mountedCell,
  onCellClosed,
  openCellField,
  openCellRange,
  setOpenCell,
} from "./openCell";
import type { GridCell, GridTable } from "./structure";
import { gridTableAt, gridTables } from "./structure";

const HOSTING = "cm-grid-hosting";

/** The main view each grid root was drawn for. */
const mainOf = new WeakMap<HTMLElement, EditorView>();
/** The table a grid root shows, or will once a waiting patch lands. */
const shownOf = new WeakMap<HTMLElement, GridTable>();
/** The resolver a grid root's chips were drawn with. */
const resolverOf = new WeakMap<HTMLElement, WikilinkResolver | undefined>();
/** The text each static cell element was drawn from. */
const drawnOf = new WeakMap<HTMLElement, string>();
/** The cell element hosting the cell editor, per main view. */
const hostOf = new WeakMap<EditorView, HTMLElement>();
/** Grid roots with a patch waiting for a composition to end. */
const waiting = new WeakSet<HTMLElement>();

function sameShape(a: GridTable, b: GridTable): boolean {
  return (
    a.rows.length === b.rows.length &&
    a.rows.every((row, r) => row.length === b.rows[r]?.length)
  );
}

function sameTexts(a: GridTable, b: GridTable): boolean {
  return a.rows.every((row, r) =>
    row.every((cell, c) => cell.text === b.rows[r]?.[c]?.text),
  );
}

function sameAligns(a: GridTable, b: GridTable): boolean {
  return (
    a.aligns.length === b.aligns.length &&
    a.aligns.every((align, i) => align === b.aligns[i])
  );
}

function holds(table: GridTable, cell: GridCell): boolean {
  return table.from <= cell.span.from && cell.span.to <= table.to;
}

/** A cell element's row (0 is the header) and column. */
export function placeOf(el: HTMLElement): [number, number] {
  return [
    Number(el.getAttribute("aria-rowindex")) - 1,
    Number(el.getAttribute("aria-colindex")) - 1,
  ];
}

function cellElements(root: HTMLElement): HTMLElement[] {
  return [
    ...root.querySelectorAll<HTMLElement>("[aria-rowindex][aria-colindex]"),
  ];
}

/** The cell element at `row` (0 is the header) and `column` of a grid root. */
export function cellElement(
  root: HTMLElement,
  row: number,
  column: number,
): HTMLElement | null {
  return root.querySelector<HTMLElement>(
    `[aria-rowindex="${String(row + 1)}"][aria-colindex="${String(column + 1)}"]`,
  );
}

/**
 * Makes `el` the grid's one tab stop (the roving `tabindex`): the cell Tab
 * comes back to and the one grid navigation starts from.
 */
export function rove(el: HTMLElement): void {
  const root = el.closest<HTMLElement>(".cm-grid");
  if (!root) return;
  for (const other of cellElements(root)) {
    const value = other === el ? 0 : -1;
    if (other.tabIndex !== value) other.tabIndex = value;
  }
}

function setAlign(el: HTMLElement, table: GridTable, column: number): void {
  const align = table.aligns[column];
  const value = align === undefined || align === "none" ? "" : align;
  if (el.style.textAlign !== value) el.style.textAlign = value;
}

/** The facet's value in a state nobody gave a resolver: no chips there. */
let noResolver: WikilinkResolver | null = null;

/**
 * The resolver static cells draw chips with, or undefined when the main view
 * draws none (R12): the engram editor provides `wikilinkResolverFacet`, a
 * Reflection does not. Told apart by the facet's value in an empty state,
 * which is its default.
 */
function chipResolver(state: EditorState): WikilinkResolver | undefined {
  noResolver ??= EditorState.create().facet(wikilinkResolverFacet);
  const resolve = state.facet(wikilinkResolverFacet);
  return resolve === noResolver ? undefined : resolve;
}

/**
 * What a static cell is drawn from besides its text: the footnote numbers of
 * its references (offsets into its text), and whether comments are left out
 * (a Reflection). Its key tells a redraw is due.
 */
interface CellExtras {
  notes: { from: number; to: number; number: number }[];
  hideComments: boolean;
  key: string;
}

function extrasOf(state: EditorState, cell: GridCell | undefined): CellExtras {
  const hideComments = hidesIdLines(state);
  const notes = cell
    ? footnoteRefsIn(state, cell.content.from, cell.content.to).map((n) => ({
        from: n.from - cell.content.from,
        to: n.to - cell.content.from,
        number: n.number,
      }))
    : [];
  const key = `${hideComments ? "c" : ""}${notes
    .map((n) => `${String(n.from)}-${String(n.to)}:${String(n.number)}`)
    .join(",")}`;
  return { notes, hideComments, key };
}

/** The numbers a table's references are drawn with, as one string. */
function notesKey(state: EditorState, table: GridTable): string {
  return footnoteRefsIn(state, table.from, table.to)
    .map((n) => `${String(n.from)}:${String(n.number)}`)
    .join(",");
}

function drawStatic(
  el: HTMLElement,
  text: string,
  resolve: WikilinkResolver | undefined,
  extras: CellExtras,
): void {
  el.classList.remove(HOSTING);
  renderCellContent(text, el, {
    ...(resolve ? { resolve } : {}),
    notes: extras.notes,
    hideComments: extras.hideComments,
  });
  drawnOf.set(el, `${extras.key}\u0000${text}`);
}

function hosts(main: EditorView, el: HTMLElement): boolean {
  return mountedCell(main)?.parent === el;
}

/**
 * Brings every static cell of `root` up to the table it should show. The
 * cell hosting the editor is left alone.
 */
function patch(root: HTMLElement, main: EditorView): void {
  waiting.delete(root);
  const table = shownOf.get(root);
  const resolve = chipResolver(main.state);
  if (!table) return;
  const redrawAll = resolverOf.has(root) && resolverOf.get(root) !== resolve;
  resolverOf.set(root, resolve);
  const label = gridLabel(table);
  const grid = root.querySelector("table");
  if (grid && grid.getAttribute("aria-label") !== label) {
    grid.setAttribute("aria-label", label);
  }
  for (const el of cellElements(root)) {
    const [row, column] = placeOf(el);
    if (hosts(main, el)) continue;
    setAlign(el, table, column);
    const cell = table.rows[row]?.[column];
    const text = cell?.text ?? "";
    const extras = extrasOf(main.state, cell);
    if (
      redrawAll ||
      drawnOf.get(el) !== `${extras.key}\u0000${text}` ||
      el.classList.contains(HOSTING)
    )
      drawStatic(el, text, resolve, extras);
  }
}

/**
 * Inline HTML tags taken out until none is left (a removal can join two
 * halves into a new tag, `<<b>script>`), then any bracket left over.
 */
function withoutTags(text: string): string {
  let rest = text;
  let before: string;
  do {
    before = rest;
    rest = rest.replace(/<[^<>]*>/g, "");
  } while (rest !== before);
  return rest.replace(/[<>]/g, "");
}

/** A cell's text as a reader hears it: drawn, then read back as plain text. */
function plainOf(text: string): string {
  // Read as text, never rendered: a header holding an image starts no fetch.
  return withoutTags(
    text
      .replace(/\\\|/g, "|") // a cell's escaped pipe, as in a wikilink label
      .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1") // an image: its alt text
      .replace(
        /\[\[([^\]|]*)(?:\|([^\]]*))?\]\]/g,
        (_m, target: string, label?: string) => label ?? target,
      ) // a wikilink: its label, else its target
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1"), // a link: its text
  )
    .replace(/(\*\*|__|~~|\*|_|`)/g, "") // emphasis and code marks
    .replace(/\\(.)/g, "$1") // escapes: the character itself
    .replace(/\s+/g, " ")
    .trim();
}

/** What a column is called: its header's text, or its number when empty. */
function columnName(table: GridTable, column: number): string {
  return (
    plainOf(table.rows[0]?.[column]?.text ?? "") ||
    `Column ${String(column + 1)}`
  );
}

/**
 * The grid's name, from its header ("Table: Name, Role, Tide"), so two
 * tables on one page can be told apart (finding 7).
 */
export function gridLabel(table: GridTable): string {
  const names = (table.rows[0] ?? []).map((_cell, c) => columnName(table, c));
  return names.length > 0 ? `Table: ${names.join(", ")}` : "Table";
}

/** The open cell editor's name: its column and its row ("Role, row 2"). */
export function cellLabel(
  table: GridTable,
  row: number,
  column: number,
): string {
  const name = columnName(table, column);
  return row === 0 ? `${name}, header` : `${name}, row ${String(row)}`;
}

/**
 * The hosted cell editor's content attributes, read again on every cell
 * update: a name that says which cell it is, and single line, because Enter
 * moves down a column rather than breaking the line.
 */
function cellAttributes(main: EditorView): Extension {
  return EditorView.contentAttributes.of(() => {
    const cell = openCellRange(main.state);
    const table = cell ? gridTableAt(main.state, cell.span.from) : null;
    return {
      "aria-multiline": "false",
      ...(cell && table
        ? { "aria-label": cellLabel(table, cell.row, cell.column) }
        : {}),
    };
  });
}

/** Draws a cell that hosted the editor and no longer does as text again. */
function restore(main: EditorView, el: HTMLElement): void {
  if (hosts(main, el)) return;
  const root = el.closest<HTMLElement>(".cm-grid");
  const table = root ? shownOf.get(root) : undefined;
  if (!root || !table) return;
  // A patch that waited for a composition the editor never finished: the
  // editor is gone, so it lands now, this cell with it.
  if (waiting.has(root)) {
    patch(root, main);
    return;
  }
  const [row, column] = placeOf(el);
  const cell = table.rows[row]?.[column];
  drawStatic(
    el,
    cell?.text ?? "",
    chipResolver(main.state),
    extrasOf(main.state, cell),
  );
}

/** Forgets and redraws the host cell once the editor in it has gone. */
function settleHost(main: EditorView): void {
  const host = hostOf.get(main);
  if (!host || hosts(main, host)) return;
  hostOf.delete(main);
  restore(main, host);
}

/**
 * Mounts the cell editor for `main`'s open cell into `el`, which must be the
 * cell the intent names, and draws the cell it left as text again. `focus`
 * is "now" for a cell already in the document, "later" for one `toDOM` is
 * still building, which CodeMirror attaches before a microtask runs.
 */
function host(
  main: EditorView,
  el: HTMLElement,
  focus: "now" | "later" | "none",
): CellEditor | null {
  if (
    !gridWritable(main.state) ||
    !main.state.field(openCellField, false) ||
    !openCellRange(main.state)
  ) {
    return null;
  }
  const previous = hostOf.get(main);
  el.replaceChildren();
  drawnOf.delete(el);
  el.classList.add(HOSTING);
  const editor = mountCellEditor(main, el, [
    cellPaste,
    cellKeymap,
    floatingCell(main),
    formatCell(main),
    elementEditCell(main),
    cellAttributes(main),
  ]);
  hostOf.set(main, el);
  rove(el);
  // The mount tore down whatever hosted before, in this table or another
  // one; that cell is text again.
  if (previous && previous !== el) restore(main, previous);
  if (focus === "now") editor.view.focus();
  else if (focus === "later") {
    queueMicrotask(() => {
      if (mountedCell(main)?.view !== editor.view || !el.isConnected) return;
      // The field's `focused` outlives a teardown that sent no focus change
      // (virtualisation): take focus only when nothing else holds it, or
      // when it is inside this grid (a shape change just rebuilt it).
      const active = document.activeElement;
      const root = el.closest(".cm-grid");
      if (
        active === null ||
        active === document.body ||
        (root !== null && root.contains(active))
      ) {
        editor.view.focus();
      }
    });
  }
  return editor;
}

/** The grid root drawn for the table holding the open cell, if on screen. */
function rootFor(main: EditorView, cell: GridCell): HTMLElement | null {
  for (const root of main.contentDOM.querySelectorAll<HTMLElement>(
    ".cm-grid",
  )) {
    const table = tableOf(main, root);
    if (table && holds(table, cell)) return root;
  }
  return null;
}

/** The table a grid root stands for, read from the current state. */
export function tableOf(main: EditorView, root: HTMLElement): GridTable | null {
  let pos: number;
  try {
    pos = main.posAtDOM(root);
  } catch {
    return null; // not one of this view's widgets (any more)
  }
  const doc = main.state.doc;
  return (
    gridTables(main.state).find(
      (table) => doc.lineAt(table.from).from === pos,
    ) ?? null
  );
}

/**
 * Mounts the cell editor where `main`'s open-cell intent points, when that
 * table is drawn: the one rule a mouse, a key or a search hit moves
 * the editor by. Returns the mounted editor, or null when the cell is not on
 * screen (the table's `toDOM` mounts it once it is).
 */
export function hostOpenCell(
  main: EditorView,
  focus: "now" | "none" = "now",
): CellEditor | null {
  const cell = openCellRange(main.state);
  if (!cell) return null;
  const root = rootFor(main, cell);
  const el = root ? cellElement(root, cell.row, cell.column) : null;
  if (!el) return null;
  if (hosts(main, el)) {
    const editor = activeCell(main);
    if (focus === "now") editor?.view.focus();
    return editor;
  }
  return host(main, el, focus);
}

/** Opens the cell `el` stands for, with the caret under `coords`. */
function openFromMouse(
  main: EditorView,
  root: HTMLElement,
  el: HTMLElement,
  coords: { x: number; y: number },
): void {
  const table = tableOf(main, root);
  const [row, column] = placeOf(el);
  const cell = table?.rows[row]?.[column];
  if (!cell) return;
  const at = cell.content.to;
  main.dispatch({
    effects: setOpenCell.of({ anchor: at, head: at, focused: true }),
  });
  const editor = host(main, el, "now");
  if (!editor) return;
  // jsdom has no layout: there `posAtCoords` finds nothing and the caret
  // stays at the end. A browser places it under the pointer.
  const pos = editor.view.posAtCoords(coords);
  if (pos === null) return;
  const { from, to } = cell.content;
  editor.view.dispatch({
    selection: { anchor: Math.min(to, Math.max(from, pos)) },
  });
}

export class GridWidget extends WidgetType {
  readonly table: GridTable;
  readonly resolve: WikilinkResolver | undefined;
  /** The footnote numbers its references are drawn with (`notesKey`). */
  readonly notes: string;

  constructor(
    table: GridTable,
    resolve: WikilinkResolver | undefined,
    notes = "",
  ) {
    super();
    this.table = table;
    this.resolve = resolve;
    this.notes = notes;
  }

  /** Same shape, same alignments, same cell texts and footnote numbers. */
  override eq(other: GridWidget): boolean {
    return (
      sameShape(this.table, other.table) &&
      sameAligns(this.table, other.table) &&
      sameTexts(this.table, other.table) &&
      this.resolve === other.resolve &&
      this.notes === other.notes
    );
  }

  override get estimatedHeight(): number {
    return this.table.rows.length * 30 + 8;
  }

  override toDOM(view: EditorView): HTMLElement {
    const root = document.createElement("div");
    root.className = "cm-grid";
    const grid = document.createElement("table");
    grid.setAttribute("role", "grid");
    grid.setAttribute("aria-label", gridLabel(this.table));
    grid.tabIndex = -1;
    const head = document.createElement("thead");
    const body = document.createElement("tbody");
    this.table.rows.forEach((row, r) => {
      const tr = document.createElement("tr");
      tr.setAttribute("role", "row");
      row.forEach((_cell, c) => {
        const el = document.createElement(r === 0 ? "th" : "td");
        el.setAttribute("role", r === 0 ? "columnheader" : "gridcell");
        el.setAttribute("aria-rowindex", String(r + 1));
        el.setAttribute("aria-colindex", String(c + 1));
        // The roving tab stop; `rove` moves it.
        el.tabIndex = r === 0 && c === 0 ? 0 : -1;
        tr.appendChild(el);
      });
      (r === 0 ? head : body).appendChild(tr);
    });
    grid.append(head, body);
    gridNavKeys(view, grid);
    root.appendChild(grid);
    mainOf.set(root, view);
    shownOf.set(root, this.table);
    patch(root, view);
    mountChrome(root, view);

    root.addEventListener("mousedown", (event) => {
      if (event.button !== 0) return;
      const target = event.target as Element;
      const el = target.closest<HTMLElement>("[aria-colindex]");
      if (!el || !root.contains(el)) return;
      if (hosts(view, el)) return; // the cell view handles its own clicks
      // Read-only: no cell opens, and the text stays selectable.
      if (!gridWritable(view.state)) return;
      // Mod-click on a link opens the link; anything else opens the cell.
      if ((event.metaKey || event.ctrlKey) && target.closest("a")) return;
      event.preventDefault();
      openFromMouse(view, root, el, { x: event.clientX, y: event.clientY });
    });

    // Remount from intent: a shape change, or the table back in view.
    const open = view.state.field(openCellField, false);
    const cell = openCellRange(view.state);
    if (open && cell && holds(this.table, cell)) {
      const el = cellElement(root, cell.row, cell.column);
      if (el) host(view, el, open.focused ? "later" : "none");
    }
    return root;
  }

  override updateDOM(
    dom: HTMLElement,
    view: EditorView,
    from: WidgetType,
  ): boolean {
    if (!(from instanceof GridWidget) || !sameShape(from.table, this.table)) {
      return false;
    }
    const mounted = mountedCell(view);
    const hostsHere = mounted !== null && dom.contains(mounted.parent);
    if (hostsHere) {
      // CodeMirror may offer one table's DOM to another of the same shape;
      // the DOM holding the editor goes only to the table holding the cell.
      const cell = openCellRange(view.state);
      if (!cell || !holds(this.table, cell)) return false;
    }
    const shown = shownOf.get(dom) ?? from.table;
    if (!sameTexts(shown, this.table) || !sameAligns(shown, this.table)) {
      tableChangedUnderChrome(dom);
    }
    shownOf.set(dom, this.table);
    if (hostsHere && mounted.view.composing) {
      if (!waiting.has(dom)) {
        waiting.add(dom);
        mounted.view.contentDOM.addEventListener(
          "compositionend",
          () => {
            if (waiting.has(dom)) patch(dom, view);
          },
          { once: true },
        );
      }
      return true;
    }
    patch(dom, view);
    return true;
  }

  override destroy(dom: HTMLElement): void {
    disposeChrome(dom);
    const main = mainOf.get(dom);
    if (!main) return;
    const mounted = mountedCell(main);
    // Not `closeCell`: the intent stays, and the next `toDOM` remounts.
    if (mounted && dom.contains(mounted.parent)) mounted.teardown();
    const host = hostOf.get(main);
    if (host && dom.contains(host)) hostOf.delete(main);
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

function build(state: EditorView["state"]): DecorationSet {
  const doc = state.doc;
  const resolve = chipResolver(state);
  const ranges: Range<Decoration>[] = gridTables(state).map((table) =>
    Decoration.replace({
      block: true,
      widget: new GridWidget(table, resolve, notesKey(state, table)),
    }).range(doc.lineAt(table.from).from, doc.lineAt(table.to).to),
  );
  return Decoration.set(ranges);
}

/** The grid's block replacements, rebuilt on doc and syntax tree changes. */
export const gridField = StateField.define<DecorationSet>({
  create: build,
  update: (value, tr) =>
    tr.docChanged ||
    parseAdvanced(tr) ||
    tr.startState.facet(wikilinkResolverFacet) !==
      tr.state.facet(wikilinkResolverFacet) ||
    // A footnote read settled: the closed cells draw its numbers.
    footnoteVersion(tr.startState) !== footnoteVersion(tr.state)
      ? build(tr.state)
      : value,
  provide: (field) => EditorView.decorations.from(field),
});

/**
 * Draws a host cell as text again when its editor went away without the grid
 * moving it: a close, or a mount somewhere else.
 */
const hostWatch = ViewPlugin.fromClass(
  class {
    readonly stop: () => void;

    constructor(view: EditorView) {
      this.stop = onCellClosed(view, () => {
        settleHost(view);
      });
    }

    update(update: ViewUpdate): void {
      if (update.docChanged) mapChromeMenus(update.view, update.changes);
      settleHost(update.view);
    }

    destroy(): void {
      this.stop();
    }
  },
);

/**
 * The look of the read-only table preview it replaces (borders, header
 * weight, padding), plus a focus ring on the focused cell. The hosted cell
 * view's own padding is taken away so its text sits where the static text did.
 */
const gridTheme = EditorView.baseTheme({
  ".cm-grid": { padding: "0.25rem 0.75rem", position: "relative" },
  ".cm-grid-chrome[hidden]": { display: "none" },
  ".cm-grid-plus, .cm-grid-cell-menu": {
    position: "absolute",
    width: "18px",
    height: "18px",
    padding: "0",
    lineHeight: "16px",
    textAlign: "center",
    fontSize: "0.8rem",
    cursor: "pointer",
    border: "1px solid var(--color-slate-300)",
    borderRadius: "9999px",
    background: "var(--color-white)",
    color: "var(--color-slate-700)",
    zIndex: "5",
  },
  ".cm-grid-cell-menu": {
    borderRadius: "4px",
    width: "16px",
    height: "16px",
    lineHeight: "14px",
  },
  ".cm-grid-menu": {
    position: "fixed",
    zIndex: "20",
    minWidth: "11rem",
    padding: "0.25rem",
    border: "1px solid var(--color-slate-300)",
    borderRadius: "6px",
    background: "var(--color-white)",
    color: "var(--color-slate-900)",
    boxShadow: "0 4px 12px rgb(0 0 0 / 0.15)",
    fontSize: "0.875rem",
  },
  ".cm-grid-menu-item": {
    padding: "0.25rem 0.5rem",
    borderRadius: "4px",
    cursor: "pointer",
  },
  ".cm-grid-menu-item:focus, .cm-grid-menu-item:hover": {
    outline: "none",
    background: "var(--color-slate-100)",
  },
  ".cm-grid-menu-item[aria-disabled='true']": {
    opacity: "0.45",
    cursor: "default",
  },
  ".cm-grid-menu-item[aria-checked='true']::after": { content: '" \u2713"' },
  "&dark .cm-grid-plus, &dark .cm-grid-cell-menu, &dark .cm-grid-menu": {
    background: "var(--color-slate-800)",
    borderColor: "var(--color-slate-600)",
    color: "var(--color-slate-100)",
  },
  "&dark .cm-grid-menu-item:focus, &dark .cm-grid-menu-item:hover": {
    background: "var(--color-slate-700)",
  },
  ".cm-grid table": { borderCollapse: "collapse" },
  ".cm-grid table:focus": { outline: "none" },
  ".cm-grid th, .cm-grid td": {
    border: "1px solid var(--color-slate-300)",
    padding: "0.15rem 0.5rem",
    textAlign: "left",
    verticalAlign: "top",
    cursor: "text",
  },
  ".cm-grid th:focus-visible, .cm-grid td:focus-visible, .cm-grid .cm-grid-hosting:focus-within":
    {
      outline: "2px solid var(--color-accent-600)",
      outlineOffset: "-2px",
    },
  "&dark .cm-grid th:focus-visible, &dark .cm-grid td:focus-visible, &dark .cm-grid .cm-grid-hosting:focus-within":
    { outlineColor: "var(--color-accent-400)" },
  ".cm-grid-hosting .cm-editor": { background: "transparent" },
  ".cm-grid-hosting .cm-editor.cm-focused": { outline: "none" },
  ".cm-grid-hosting .cm-content": { padding: "0" },
  ".cm-grid-hosting .cm-line": { padding: "0" },
});

/** The grid's view side: the field, the host watch and the theme. */
export const gridView: Extension = [gridField, hostWatch, gridTheme];
