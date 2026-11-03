/**
 * Element edit requests: a click on a drawn link, chip, status pill, formula
 * or callout kind icon, a right click on a task box, Mod-k, Shift+F10 or the
 * context-menu key asks for a popover or a menu, and the screen's lazy host
 * (`popovers/ElementPopover.tsx`) answers it. Each kind has its own reader
 * (`spanAt`), so a request follows its own element and nothing else.
 *
 * A request is a state effect on the view it came from. Per main view one hub
 * holds the open request and the view that sent it: main itself, or the open
 * table cell, whose side reaches the hub through `elementEditCell(main)`,
 * which the grid adds to every cell editor it mounts (like `floatingCell`).
 * The hub maps the request through every change of that view, so a commit
 * writes where the element is now, and closes it without writing when the
 * element is gone (a peer deleted it), the view is gone, the setting or Raw
 * flipped, the buffer was rebuilt or the page stopped taking writes.
 *
 * A commit writes on the view that asked, as one gesture: one undo step solo
 * and in a room, `userEvent: "input"`, and only the span the popover edits.
 * Escape or any close other than Save writes nothing.
 *
 * Eager within the editor chunk and free of `grid/` imports, like
 * `floatingContext.ts`.
 */

import { isolateHistory } from "@codemirror/commands";
import type {
  ChangeSpec,
  EditorState,
  Extension,
  Transaction,
} from "@codemirror/state";
import {
  EditorSelection,
  Facet,
  MapMode,
  Prec,
  StateEffect,
} from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin, keymap } from "@codemirror/view";

import type { LinkElement, Span } from "./elementVerbs";
import {
  calloutMarkAt,
  linkAt,
  mathAt,
  newLinkChanges,
  statusAt,
  taskAt,
  wikilinkAt,
} from "./elementVerbs";
import {
  hiddenMarkLayerOn,
  markModeChanged,
  marksHiddenOnLine,
} from "./marks/mode";
import { roomUndo } from "./roomUndo";
import { insertMarkdownLink } from "./toolbar";

export type ElementEdit =
  | {
      kind: "link";
      at: number;
      rect: DOMRect;
      text: Span;
      address: Span | null;
      reference: boolean;
    }
  | { kind: "new-link"; rect: DOMRect; range: Span }
  | { kind: "wikilink"; at: number; rect: DOMRect; target: Span }
  | { kind: "status"; at: number; rect: DOMRect }
  | { kind: "math"; at: number; rect: DOMRect; block: boolean }
  | { kind: "task"; at: number; rect: DOMRect }
  | { kind: "callout-kind"; at: number; rect: DOMRect }
  | { kind: "language"; at: number; rect: DOMRect };

/**
 * A Ctrl+click on a Mac: the browser opens its context menu for it, so no
 * drawn element acts on the press (no write, no popover, no new tab).
 */
export function macMenuClick(event: MouseEvent): boolean {
  return event.ctrlKey && /Mac/.test(navigator.platform);
}

/** Where the element a request names starts. */
export function editAt(edit: ElementEdit): number {
  return edit.kind === "new-link" ? edit.range.from : edit.at;
}

/**
 * Whether this document links engrams with `[[...]]`: provided by the chip
 * layer, so a Reflection page (no chips) opens no wikilink popover.
 */
export const linksEngrams = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/** Opens a popover for an element (or closes the open one, with null). */
export const elementEditRequest = StateEffect.define<ElementEdit | null>();

/** The open request and the view that sent it. */
export interface ElementRequest {
  edit: ElementEdit;
  view: EditorView;
}

interface Hub {
  main: EditorView;
  onChange: (edit: ElementEdit | null, view: EditorView) => void;
  current: ElementRequest | null;
}

const hubs = new WeakMap<EditorView, Hub>();

/** The main view each mounted cell view reports to. */
const cellMains = new WeakMap<EditorView, EditorView>();

function hubOf(view: EditorView): Hub | undefined {
  const main = cellMains.get(view) ?? view;
  return hubs.get(main);
}

function isDestroyed(view: EditorView): boolean {
  return (view as unknown as { destroyed?: unknown }).destroyed === true;
}

/** The R17 test (C12): a page that is not writable takes no popover edit. */
export function writable(state: EditorState): boolean {
  return state.facet(EditorView.editable) && !state.readOnly;
}

function tell(hub: Hub, next: ElementRequest | null): void {
  const was = hub.current;
  hub.current = next;
  if (next === null && was === null) {
    return;
  }
  hub.onChange(next?.edit ?? null, next?.view ?? hub.main);
}

/**
 * The element `edit` names in `state`, re-read at `at`: its fresh spans, or
 * null when it is not there any more. Each kind reads with its own reader,
 * so a request never turns into another kind of element.
 */
function reread(
  state: EditorState,
  edit: Exclude<ElementEdit, { kind: "new-link" }>,
  at: number,
): ElementEdit | null {
  switch (edit.kind) {
    case "link": {
      const link = linkAt(state, at);
      return link ? linkEdit(link, edit.rect) : null;
    }
    case "wikilink": {
      const link = wikilinkAt(state, at);
      return link
        ? {
            kind: "wikilink",
            at: link.from,
            rect: edit.rect,
            target: link.target,
          }
        : null;
    }
    case "math": {
      const math = mathAt(state, at);
      return math ? { ...edit, at: math.from, block: math.block } : null;
    }
    default: {
      const span = spanAt(state, edit.kind, at);
      return span ? { ...edit, at: span.from } : null;
    }
  }
}

/** Where the element of kind `kind` at `at` spans in `state`, or null. */
function spanAt(
  state: EditorState,
  kind: Exclude<ElementEdit["kind"], "new-link">,
  at: number,
): Span | null {
  switch (kind) {
    case "link":
      return linkAt(state, at);
    case "wikilink":
      return wikilinkAt(state, at);
    case "status":
      return statusAt(state, at);
    case "math":
      return mathAt(state, at);
    case "task":
      return taskAt(state, at);
    case "callout-kind":
      return calloutMarkAt(state, at);
    case "language":
      return { from: at, to: at };
  }
}

/**
 * `edit` carried through `tr`'s changes, or null when its element went. Both
 * ends are mapped, and the element read again must be the same kind and
 * start and end exactly there: a deletion that takes the element away must
 * never leave the request standing on the element next to it.
 */
function mapEdit(edit: ElementEdit, tr: Transaction): ElementEdit | null {
  const { changes } = tr;
  if (edit.kind === "new-link") {
    const from = changes.mapPos(edit.range.from, 1, MapMode.TrackDel);
    const to = changes.mapPos(edit.range.to, -1, MapMode.TrackDel);
    if (from === null || to === null || from > to) {
      return null;
    }
    // A selection a peer deleted is gone, not a caret now.
    if (from === to && edit.range.from !== edit.range.to) {
      return null;
    }
    return { ...edit, range: { from, to } };
  }
  if (edit.kind === "language") {
    const at = changes.mapPos(edit.at, 1, MapMode.TrackDel);
    return at === null ? null : { ...edit, at };
  }
  const before = spanAt(tr.startState, edit.kind, edit.at);
  if (!before) {
    return null;
  }
  const from = changes.mapPos(before.from, 1, MapMode.TrackDel);
  const to = changes.mapPos(before.to, -1, MapMode.TrackDel);
  if (from === null || to === null || from >= to) {
    return null;
  }
  const next = reread(tr.state, edit, from);
  const after =
    next && next.kind !== "new-link" ? spanAt(tr.state, next.kind, from) : null;
  return after && after.from === from && after.to === to ? next : null;
}

/** What every view a hub hears from does with one update. */
function follow(hub: Hub, view: EditorView, update: ViewUpdate): void {
  for (const tr of update.transactions) {
    const current = hub.current;
    if (current && current.view === view && tr.docChanged) {
      const mapped = mapEdit(current.edit, tr);
      if (mapped) {
        hub.current = { edit: mapped, view };
      } else {
        tell(hub, null);
      }
    }
    for (const effect of tr.effects) {
      if (effect.is(elementEditRequest)) {
        tell(hub, effect.value ? { edit: effect.value, view } : null);
      }
    }
    // The setting, Raw or the page's writability changing on the view that
    // asked, or on main whichever view asked: main is the page.
    if (
      hub.current &&
      (hub.current.view === view || view === hub.main) &&
      (markModeChanged(tr) || !writable(tr.state))
    ) {
      tell(hub, null);
    }
  }
}

/**
 * Tells `onChange` the element edit requested on the main view this sits in
 * or in its open cell (with the view that asked), and null when it closes.
 */
export function elementEditListener(
  onChange: (edit: ElementEdit | null, view: EditorView) => void,
): Extension {
  return ViewPlugin.fromClass(
    class {
      readonly hub: Hub;

      constructor(view: EditorView) {
        this.hub = { main: view, onChange, current: null };
        hubs.set(view, this.hub);
      }

      update(update: ViewUpdate): void {
        follow(this.hub, update.view, update);
      }

      destroy(): void {
        // A rebuilt buffer (`setState`) or a closed editor: nothing to edit.
        if (hubs.get(this.hub.main) === this.hub) {
          hubs.delete(this.hub.main);
        }
        tell(this.hub, null);
      }
    },
  );
}

/**
 * The cell view's side, added by the grid to every cell editor it mounts on
 * `main`: a request in the cell reaches `main`'s listener with the cell's
 * view, and a commit lands in the cell. A request from a cell closes when the
 * cell goes.
 */
export function elementEditCell(main: EditorView): Extension {
  return ViewPlugin.fromClass(
    class {
      readonly view: EditorView;

      constructor(view: EditorView) {
        this.view = view;
        cellMains.set(view, main);
      }

      update(update: ViewUpdate): void {
        const hub = hubs.get(main);
        if (hub) follow(hub, this.view, update);
      }

      destroy(): void {
        const hub = hubs.get(main);
        if (hub?.current?.view === this.view) tell(hub, null);
      }
    },
  );
}

/** Whether a request on `view` reaches a listener that answers it. */
export function answersElementEdits(view: EditorView): boolean {
  return hubOf(view) !== undefined;
}

/** Asks for the popover of `edit` on `view`. Nothing on a page that takes no writes. */
export function requestElementEdit(view: EditorView, edit: ElementEdit): void {
  if (!writable(view.state) || isDestroyed(view)) {
    return;
  }
  view.dispatch({ effects: elementEditRequest.of(edit) });
}

/** The open request on `main` (or in its cell), mapped to the document as it is now. */
export function openElementEdit(main: EditorView): ElementRequest | null {
  return hubs.get(main)?.current ?? null;
}

/** Closes the open request on `main` without writing. */
export function closeElementEdit(main: EditorView): void {
  const hub = hubs.get(main);
  if (hub) tell(hub, null);
}

/** Where the element `edit` names ends in `state`, or null when it is gone. */
function elementEnd(state: EditorState, edit: ElementEdit): number | null {
  return edit.kind === "new-link"
    ? edit.range.to
    : (spanAt(state, edit.kind, edit.at)?.to ?? null);
}

/**
 * Writes what `write` makes of the open request, as one gesture on the view
 * that asked, then hands that view the focus with the caret after the
 * element. Nothing is written when the request is gone, the view is gone, the
 * page takes no writes, or `write` refuses (null); an empty change list
 * writes nothing either. Closes the request in every case. Returns whether
 * anything was written.
 */
export function commitElementEdit(
  main: EditorView,
  write: (state: EditorState, edit: ElementEdit) => ChangeSpec[] | null,
): boolean {
  const request = openElementEdit(main);
  closeElementEdit(main);
  if (!request || isDestroyed(request.view)) {
    return false;
  }
  const { view, edit } = request;
  const { state } = view;
  const end = elementEnd(state, edit);
  // Main is the page: a cell's commit is forwarded there, so both must take it.
  const open = writable(state) && writable(main.state);
  const changes = open && end !== null ? write(state, edit) : null;
  if (changes === null || changes.length === 0) {
    view.focus();
    return false;
  }
  const set = state.changes(changes);
  const after = set.mapPos(end ?? 0, 1);
  const room = state.facet(roomUndo);
  // A gesture, not typing: one undo step of its own, solo and in a room.
  room?.stopCapturing();
  view.dispatch({
    changes: set,
    selection: EditorSelection.cursor(after),
    userEvent: "input",
    annotations: isolateHistory.of("full"),
    scrollIntoView: true,
  });
  room?.stopCapturing();
  view.focus();
  return true;
}

/** Closes the open request without writing and hands its view the focus back. */
export function discardElementEdit(main: EditorView): void {
  const request = openElementEdit(main);
  closeElementEdit(main);
  if (request && !isDestroyed(request.view)) {
    request.view.focus();
  }
}

const EMPTY_RECT = { left: 0, top: 0, right: 0, bottom: 0 };

/** The box around `from`..`to` in `view`, or an empty one with no layout. */
export function rectOf(view: EditorView, from: number, to: number): DOMRect {
  let box = EMPTY_RECT;
  try {
    const start = view.coordsAtPos(from, 1);
    const end = from === to ? start : view.coordsAtPos(to, -1);
    if (start && end) {
      box = {
        left: Math.min(start.left, end.left),
        top: Math.min(start.top, end.top),
        right: Math.max(start.right, end.right),
        bottom: Math.max(start.bottom, end.bottom),
      };
    }
  } catch {
    // No layout to ask (jsdom).
  }
  return new DOMRect(
    box.left,
    box.top,
    Math.max(0, box.right - box.left),
    Math.max(0, box.bottom - box.top),
  );
}

function linkEdit(link: LinkElement, rect: DOMRect): ElementEdit {
  return {
    kind: "link",
    at: link.from,
    rect,
    text: link.text,
    address: link.address,
    reference: link.reference,
  };
}

/** The element at `pos` as a request, or null. `rect` defaults to its box. */
export function elementEditAt(
  view: EditorView,
  pos: number,
  rect?: DOMRect,
): ElementEdit | null {
  const { state } = view;
  const box = (span: Span) => rect ?? rectOf(view, span.from, span.to);
  const link = linkAt(state, pos);
  if (link) {
    return linkEdit(link, box(link));
  }
  if (state.facet(linksEngrams)) {
    const wikilink = wikilinkAt(state, pos);
    if (wikilink) {
      return {
        kind: "wikilink",
        at: wikilink.from,
        rect: box(wikilink),
        target: wikilink.target,
      };
    }
  }
  const status = statusAt(state, pos);
  if (status) {
    return { kind: "status", at: status.from, rect: box(status) };
  }
  const math = mathAt(state, pos);
  if (math) {
    return { kind: "math", at: math.from, rect: box(math), block: math.block };
  }
  const task = taskAt(state, pos);
  if (task) {
    return { kind: "task", at: task.from, rect: box(task) };
  }
  // A callout's kind from its head line only: the body is prose, where the
  // key goes on.
  const callout = calloutMarkAt(state, pos);
  if (
    callout &&
    state.doc.lineAt(callout.from).number === state.doc.lineAt(pos).number
  ) {
    return { kind: "callout-kind", at: callout.from, rect: box(callout) };
  }
  return null;
}

/** What Shift+F10 at the caret opens, or null. */
export function elementAtCaret(state: EditorState): ElementEdit["kind"] | null {
  const head = state.selection.main.head;
  if (linkAt(state, head)) {
    return "link";
  }
  if (state.facet(linksEngrams) && wikilinkAt(state, head)) {
    return "wikilink";
  }
  if (statusAt(state, head)) {
    return "status";
  }
  if (mathAt(state, head)) {
    return "math";
  }
  if (taskAt(state, head)) {
    return "task";
  }
  const callout = calloutMarkAt(state, head);
  return callout &&
    state.doc.lineAt(callout.from).number === state.doc.lineAt(head).number
    ? "callout-kind"
    : null;
}

/**
 * A press on a drawn element (a link's words, a pill, a formula, a callout's
 * kind icon) that asks for its popover with `edit`, read at press time. With
 * marks hidden on its line the press places no caret (it is prevented, and
 * true is returned); with the setting to show them CodeMirror places the
 * caret and takes the focus first, so the request waits for that, and goes
 * only if the view still takes writes then.
 */
export function pressElement(
  view: EditorView,
  event: MouseEvent,
  edit: ElementEdit,
  pos: number,
): boolean {
  const { state } = view;
  if (marksHiddenOnLine(state, state.doc.lineAt(pos).number)) {
    requestElementEdit(view, edit);
    event.preventDefault();
    return true;
  }
  queueMicrotask(() => {
    requestElementEdit(view, edit);
  });
  return false;
}

/** Opens the popover of the element at the caret; false with none there. */
function openAtCaret(view: EditorView): boolean {
  if (!writable(view.state) || !answersElementEdits(view)) {
    return false;
  }
  const edit = elementEditAt(view, view.state.selection.main.head);
  if (!edit) {
    return false;
  }
  requestElementEdit(view, edit);
  return true;
}

/**
 * Shift+F10 and the context-menu key: the popover of the element at the
 * caret. With none there the key goes on (in a table cell, to Part 1's Cell
 * actions; in prose, to the browser).
 */
export const elementMenuKey: Extension = Prec.highest(
  keymap.of([
    { key: "Shift-F10", run: openAtCaret },
    { key: "ContextMenu", run: openAtCaret },
  ]),
);

/** Said when no link can go where Mod-k or the Link button asked for one. */
export const CANNOT_LINK = "A link cannot go here.";

/**
 * The Link verb where marks are hidden (Mod-k and the bars' Link button):
 * over a selection a new link for it; at a caret in a link that link; at a
 * caret in a word a new link for the word; else a new link at the caret.
 * Where a new link cannot go (code, the frontmatter, an id line, across
 * lines) nothing is written and `CANNOT_LINK` is announced.
 */
export function editLink(view: EditorView): boolean {
  const { state } = view;
  if (!writable(state)) {
    return true;
  }
  const range = state.selection.main;
  if (range.empty) {
    const link = linkAt(state, range.head);
    if (link) {
      requestElementEdit(
        view,
        linkEdit(link, rectOf(view, link.from, link.to)),
      );
      return true;
    }
  }
  const word = range.empty ? state.wordAt(range.head) : null;
  const target = word ? { from: word.from, to: word.to } : range;
  // The same test the commit makes, with a stand-in address. Refused, the
  // key is still taken (the browser's own Ctrl-K stays shut) and said.
  if (!newLinkChanges(state, target, { text: "", address: "x" })) {
    view.dispatch({ effects: EditorView.announce.of(CANNOT_LINK) });
    return true;
  }
  requestElementEdit(view, {
    kind: "new-link",
    rect: rectOf(view, target.from, target.to),
    range: { from: target.from, to: target.to },
  });
  return true;
}

/**
 * The bars' Link button on the view a verb acts on: the popover where marks
 * are hidden and a listener answers, else (Raw, a table cell before it has
 * the layer, a surface with no popover host) `insertMarkdownLink`.
 */
export function linkVerb(view: EditorView): boolean {
  if (!view.state.facet(hiddenMarkLayerOn) || !answersElementEdits(view)) {
    return insertMarkdownLink(view);
  }
  editLink(view);
  return true;
}

/**
 * Mod-k where the popover cannot answer (Raw, a table cell before it has the
 * layer, a surface with no popover host): `insertMarkdownLink`, as the Link
 * button there. Sits outside the preview compartment, in `formatKeymap`.
 */
export function plainLinkKey(view: EditorView): boolean {
  if (
    !writable(view.state) ||
    (view.state.facet(hiddenMarkLayerOn) && answersElementEdits(view))
  ) {
    return false;
  }
  return insertMarkdownLink(view);
}

/** Mod-k where marks are hidden. */
export const linkKey: Extension = Prec.highest(
  keymap.of([
    {
      key: "Mod-k",
      run: (view) => answersElementEdits(view) && editLink(view),
    },
  ]),
);
