/**
 * Presence inside the grid, both ways.
 *
 * - **`gridPresence`** (main view): the main view's own remote carets are
 *   drawn inside the text the grid replaces, so nobody sees them. This draws
 *   a marker in the cell element of every peer whose caret sits in a static
 *   cell, with the peer's colour and name the way y-codemirror.next draws a
 *   caret. It listens to the awareness `change` itself, so a peer moving
 *   needs no main-view update, and it touches only the cells a peer left
 *   or entered. After every main update it puts back the markers a redraw of
 *   their cell took away.
 * - **`cellPresence`** (cell view): the peers' carets inside the open cell,
 *   as widgets, and the local caret published to the room as relative
 *   positions while the cell has the focus. A blur or the cell view going
 *   away takes back what it published; `closeCell` clears it too. The main
 *   view publishes nothing meanwhile: it does not have the focus.
 *
 * Both read the room from y-codemirror.next's `ySyncFacet` on the main state,
 * and both use its `cursor: { anchor, head }` format.
 */

import type { EditorState, Extension } from "@codemirror/state";
import { Annotation } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";
import { ySyncFacet } from "y-codemirror.next";
import type { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { canonical } from "../marks/caret";
import { parseAdvanced } from "../parseProgress";
import {
  cellMain,
  inUpdate,
  isDestroyed,
  mountedCell,
  onCellClosed,
  openCellField,
  openCellRange,
} from "./openCell";
import type { GridCell, GridTable } from "./structure";
import { cellAt, gridTables } from "./structure";

/** What an awareness `change` event reports. */
interface Change {
  added: number[];
  updated: number[];
  removed: number[];
}

interface Room {
  awareness: Awareness;
  ytext: Y.Text;
  doc: Y.Doc;
}

/** The room `state` is in, or null outside one. */
function roomOf(state: EditorState): Room | null {
  // y-codemirror.next types its config's fields as `any`.
  const sync = state.facet(ySyncFacet) as unknown as
    { ytext: Y.Text | null; awareness: Awareness } | undefined;
  const ytext = sync?.ytext ?? null;
  const doc = ytext?.doc ?? null;
  if (!sync || !ytext || !doc) return null;
  return { awareness: sync.awareness, ytext, doc };
}

interface Peer {
  client: number;
  anchor: number;
  head: number;
  color: string;
  name: string;
}

interface CursorState {
  cursor?: { anchor?: unknown; head?: unknown } | null;
  user?: { color?: unknown; name?: unknown } | null;
}

function absolute(room: Room, json: unknown): number | null {
  if (json == null) return null;
  const abs = Y.createAbsolutePositionFromRelativePosition(
    Y.createRelativePositionFromJSON(json),
    room.doc,
  );
  return abs && abs.type === room.ytext ? abs.index : null;
}

/** One peer's caret, read from its awareness state, or null. */
function peerOf(room: Room, client: number, raw: unknown): Peer | null {
  if (client === room.doc.clientID) return null;
  const state = raw as CursorState | undefined;
  const anchor = absolute(room, state?.cursor?.anchor);
  const head = absolute(room, state?.cursor?.head);
  if (anchor === null || head === null) return null;
  const user = state?.user ?? {};
  return {
    client,
    anchor,
    head,
    color: typeof user.color === "string" ? user.color : "#30bced",
    name: typeof user.name === "string" ? user.name : "Anonymous",
  };
}

/**
 * A caret built the way y-codemirror.next's caret widget is (so its theme
 * applies), marked `cm-grid-presence`. Colour and name are set as
 * properties, never parsed as markup.
 */
function caretElement(color: string, name: string): HTMLElement {
  const caret = document.createElement("span");
  caret.className = "cm-ySelectionCaret cm-grid-presence";
  caret.setAttribute("aria-hidden", "true");
  // Copying a cell's text never takes a peer's name with it.
  caret.style.userSelect = "none";
  caret.style.backgroundColor = color;
  caret.style.borderColor = color;
  const dot = document.createElement("div");
  dot.className = "cm-ySelectionCaretDot";
  const info = document.createElement("div");
  info.className = "cm-ySelectionInfo";
  info.textContent = name;
  caret.append("⁠", dot, "⁠", info, "⁠");
  return caret;
}

/** Whether the awareness change touched anyone but this client. */
function remoteChange(
  room: Room,
  { added, updated, removed }: Change,
): boolean {
  return [...added, ...updated, ...removed].some(
    (id) => id !== room.doc.clientID,
  );
}

/** The cell element drawn for `cell` of `table`, if the table is drawn. */
function elementOf(
  main: EditorView,
  table: GridTable,
  cell: GridCell,
): HTMLElement | null {
  const start = main.state.doc.lineAt(table.from).from;
  for (const root of main.contentDOM.querySelectorAll<HTMLElement>(
    ".cm-grid",
  )) {
    let pos: number;
    try {
      pos = main.posAtDOM(root);
    } catch {
      continue;
    }
    if (pos !== start) continue;
    return root.querySelector<HTMLElement>(
      `[aria-rowindex="${String(cell.row + 1)}"][aria-colindex="${String(cell.column + 1)}"]`,
    );
  }
  return null;
}

/** The grid's tables per state, so one state is walked once. */
const tablesCache = new WeakMap<EditorState, GridTable[]>();

function tablesOf(state: EditorState): GridTable[] {
  let tables = tablesCache.get(state);
  if (!tables) {
    tables = gridTables(state);
    tablesCache.set(state, tables);
  }
  return tables;
}

interface Drawn {
  el: HTMLElement;
  color: string;
  name: string;
}

class GridPresence {
  readonly main: EditorView;
  readonly room: Room | null;
  /** The marker each peer has, by client id. */
  readonly drawn = new Map<number, Drawn>();
  readonly listener = (change: Change) => {
    if (!this.room || !remoteChange(this.room, change)) return;
    const { added, updated, removed } = change;
    this.draw([...added, ...updated, ...removed]);
  };

  readonly stopClosed: () => void;

  constructor(main: EditorView) {
    this.main = main;
    this.room = roomOf(main.state);
    this.room?.awareness.on("change", this.listener);
    // A close may give a host cell back after main's last update.
    this.stopClosed = onCellClosed(main, () => {
      queueMicrotask(() => {
        this.draw();
      });
    });
  }

  /** Whether a redraw took a marker out of the document. */
  detached(): boolean {
    for (const { el } of this.drawn.values()) if (!el.isConnected) return true;
    return false;
  }

  /**
   * Brings the markers of `clients` (every peer when omitted) up to where
   * their carets are. A marker already in the right cell is left alone, so
   * only the cells a peer left or entered change.
   */
  draw(clients?: number[]): void {
    const room = this.room;
    const main = this.main;
    if (!room || isDestroyed(main)) return;
    const states = room.awareness.getStates();
    const ids = clients ?? [
      ...new Set([...states.keys(), ...this.drawn.keys()]),
    ];
    // Drawn text only matches the document between updates.
    const synced = room.ytext.length === main.state.doc.length;
    // Read the tables only once a peer has a caret to place.
    let tables: GridTable[] | null = null;
    for (const client of ids) {
      if (client === room.doc.clientID) continue;
      const peer = synced ? peerOf(room, client, states.get(client)) : null;
      if (peer) tables ??= tablesOf(main.state);
      const target = peer && tables ? this.targetOf(tables, peer.head) : null;
      const drawn = this.drawn.get(client);
      if (!peer || !target) {
        drawn?.el.remove();
        this.drawn.delete(client);
        continue;
      }
      if (
        drawn &&
        drawn.el.parentElement === target &&
        drawn.color === peer.color &&
        drawn.name === peer.name
      ) {
        continue;
      }
      drawn?.el.remove();
      const el = caretElement(peer.color, peer.name);
      target.appendChild(el);
      this.drawn.set(client, { el, color: peer.color, name: peer.name });
    }
  }

  /** The static cell element a caret at `head` should be drawn in. */
  targetOf(tables: GridTable[], head: number): HTMLElement | null {
    const table = tables.find((t) => t.from <= head && head <= t.to);
    const cell = table ? cellAt(table, head) : null;
    if (!table || !cell) return null;
    const el = elementOf(this.main, table, cell);
    // The cell view draws the carets in the cell it edits.
    if (!el || mountedCell(this.main)?.parent === el) return null;
    return el;
  }

  destroy(): void {
    this.stopClosed();
    this.room?.awareness.off("change", this.listener);
    for (const { el } of this.drawn.values()) el.remove();
    this.drawn.clear();
  }
}

const gridPresencePlugin = ViewPlugin.fromClass(GridPresence);

/**
 * The main-view side. The redraw runs in an update listener, after the grid
 * has drawn its cells for the update (a `toDOM`, a patch, a cell drawn as
 * text again after a close), never in a plugin's `update`, which runs before
 * the DOM does.
 */
export const gridPresence: Extension = [
  gridPresencePlugin,
  EditorView.updateListener.of((update) => {
    const plugin = update.view.plugin(gridPresencePlugin);
    if (!plugin) return;
    // Only an update that can move a cell or redraw one: the document, the
    // viewport, the parse (tables drawn), the open cell (a host taken or
    // given back), or a marker a redraw took away.
    if (
      update.docChanged ||
      update.viewportChanged ||
      parseAdvanced(update) ||
      update.startState.field(openCellField, false) !==
        update.state.field(openCellField, false) ||
      plugin.detached()
    ) {
      plugin.draw();
    }
  }),
];

class CaretWidget extends WidgetType {
  readonly color: string;
  readonly name: string;

  constructor(color: string, name: string) {
    super();
    this.color = color;
    this.name = name;
  }

  override eq(other: CaretWidget): boolean {
    return other.color === this.color && other.name === this.name;
  }

  override toDOM(): HTMLElement {
    return caretElement(this.color, this.name);
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

/** Marks a cell-view transaction that only asks presence to redraw. */
const redraw = Annotation.define<boolean>();

interface Published {
  anchor: Y.RelativePosition;
  head: Y.RelativePosition;
}

function samePosition(a: unknown, b: Y.RelativePosition): boolean {
  if (a == null) return false;
  return Y.compareRelativePositions(Y.createRelativePositionFromJSON(a), b);
}

class CellPresence {
  readonly view: EditorView;
  readonly main: EditorView | null;
  readonly room: Room | null;
  decorations: DecorationSet = Decoration.none;
  /** What this view published as the local cursor, if anything. */
  published: Published | null = null;
  pending = false;
  readonly listener = (change: Change) => {
    if (this.room && remoteChange(this.room, change)) this.request();
  };
  readonly onFocus = () => {
    this.publish();
  };
  readonly onBlur = () => {
    this.takeBack();
  };

  constructor(view: EditorView) {
    this.view = view;
    this.main = view.state.facet(cellMain);
    this.room = this.main ? roomOf(this.main.state) : null;
    if (!this.room) return;
    this.room.awareness.on("change", this.listener);
    view.contentDOM.addEventListener("focus", this.onFocus);
    view.contentDOM.addEventListener("blur", this.onBlur);
    this.decorations = this.build();
  }

  /** Whether the cell, main and the shared text hold the same document. */
  synced(): boolean {
    const main = this.main;
    if (!main || !this.room) return false;
    return (
      this.view.state.doc.eq(main.state.doc) &&
      this.room.ytext.length === main.state.doc.length
    );
  }

  /** Redraws and republishes once the views agree, outside any update. */
  request(): void {
    if (this.pending) return;
    this.pending = true;
    queueMicrotask(() => {
      this.pending = false;
      if (isDestroyed(this.view)) return;
      if (inUpdate(this.view) || (this.main && inUpdate(this.main))) {
        this.request();
        return;
      }
      this.view.dispatch({ annotations: redraw.of(true) });
    });
  }

  update(update: ViewUpdate): void {
    if (!this.room) return;
    const asked = update.transactions.some(
      (tr) => tr.annotation(redraw) || tr.reconfigured,
    );
    if (!update.docChanged && !update.selectionSet && !asked) return;
    if (!this.synced()) {
      // A local change main has not taken yet: keep the carets where they
      // are and look again once it has.
      this.decorations = this.decorations.map(update.changes);
      this.request();
      return;
    }
    this.decorations = this.build();
    this.publish();
  }

  /** The peers' carets inside the open cell's content. */
  build(): DecorationSet {
    const room = this.room;
    const main = this.main;
    if (!room || !main || !this.synced()) return Decoration.none;
    const content = openCellRange(main.state)?.content;
    if (!content) return Decoration.none;
    const widgets = [];
    for (const [client, raw] of room.awareness.getStates()) {
      const peer = peerOf(room, client, raw);
      if (!peer || peer.head < content.from || peer.head > content.to) continue;
      // Drawn at its visible place: a head inside hidden marks would sit in
      // a replaced range and not be drawn at all.
      const at = canonical(this.view.state, peer.head);
      widgets.push(
        Decoration.widget({
          widget: new CaretWidget(peer.color, peer.name),
          side: peer.head - peer.anchor > 0 ? -1 : 1,
        }).range(at),
      );
    }
    return Decoration.set(widgets, true);
  }

  /** Publishes the local caret while the cell has the focus. */
  publish(): void {
    const room = this.room;
    if (!room || !this.view.hasFocus || !this.synced()) return;
    const local = room.awareness.getLocalState() as CursorState | null;
    if (local === null) return;
    const { anchor, head } = this.view.state.selection.main;
    const next: Published = {
      anchor: Y.createRelativePositionFromTypeIndex(room.ytext, anchor),
      head: Y.createRelativePositionFromTypeIndex(room.ytext, head),
    };
    this.published = next;
    if (
      samePosition(local.cursor?.anchor, next.anchor) &&
      samePosition(local.cursor?.head, next.head)
    ) {
      return;
    }
    room.awareness.setLocalStateField("cursor", next);
  }

  /** Clears the local cursor, when it is still the one this view set. */
  takeBack(): void {
    const room = this.room;
    const mine = this.published;
    this.published = null;
    if (!room || !mine) return;
    const local = room.awareness.getLocalState() as CursorState | null;
    const cursor = local?.cursor;
    if (
      cursor != null &&
      samePosition(cursor.anchor, mine.anchor) &&
      samePosition(cursor.head, mine.head)
    ) {
      room.awareness.setLocalStateField("cursor", null);
    }
  }

  destroy(): void {
    if (!this.room) return;
    this.room.awareness.off("change", this.listener);
    this.view.contentDOM.removeEventListener("focus", this.onFocus);
    this.view.contentDOM.removeEventListener("blur", this.onBlur);
    this.takeBack();
  }
}

/** The cell-view side: peers' carets in the cell, the local caret out. */
export const cellPresence: Extension = ViewPlugin.fromClass(CellPresence, {
  decorations: (value) => value.decorations,
});
