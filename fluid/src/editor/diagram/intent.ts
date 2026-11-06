/**
 * What a person is doing on a drawn diagram: which element, in which mode. A
 * state field, so it maps through changes, in a room too, and comes back with
 * the widget when a fence scrolls away and returns.
 *
 * `closeDiagramIntent` is the one teardown path. A fence that stops being drawn
 * ("gone"), Raw on ("raw"), `setState` ("swap") and the view going away
 * ("destroy") all come through it, whatever noticed them first. It runs a
 * reason's `beforeClose` hooks, empties the field, tells the listeners once and
 * then places the focus.
 *
 * It never runs inside an update. Everything that notices a teardown during one
 * (the verdict watcher, the plugin below) waits for a microtask.
 *
 * The record kept beside the field is what makes this work: Raw takes the field
 * away with the preview compartment and `setState` replaces it, so what was open
 * and its draft live in a map keyed by view (pre-flight proposal 14). Task 11
 * keeps the draft current with `noteIntentDraft`.
 */

import type { EditorState } from "@codemirror/state";
import {
  EditorSelection,
  MapMode,
  StateEffect,
  StateField,
} from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { ViewPlugin } from "@codemirror/view";

import { writable } from "../elementEdit";
import { fenceAt } from "./fence";
import { diagramField } from "./field";
import type { EdgeKey } from "./flowchart/model";
import type { DiagramHost } from "./widget";
import { hostOf, onBoxMounted } from "./widget";

export type FlowTarget =
  | { kind: "canvas" }
  | { kind: "node"; id: string }
  | { kind: "edge"; key: EdgeKey };

/** 4b widens this with the sequence diagram's targets. */
export type TargetKey = FlowTarget;

export type IntentMode = "focus" | "menu" | "rename" | "connect";

export interface DiagramIntent {
  /** The fence's `from`: any position in the fence is accepted and moved there. */
  anchor: number;
  target: TargetKey;
  mode: IntentMode;
  draft?: string;
}

export type IntentCloseReason = "user" | "gone" | "raw" | "swap" | "destroy";

/** Where the focus goes after the intent closed. */
export type IntentFocus = "main-after" | "none";

/** Opens (or with null closes, without any teardown) the intent. Prefer `closeDiagramIntent`. */
export const setDiagramIntent = StateEffect.define<DiagramIntent | null>();

/**
 * Whether the state still allows the intent, in its final form: a fence is
 * drawn at the anchor, and a page that takes no writes has no menu, rename or
 * connect (pre-flight proposal 13), so such a mode falls back to the focus.
 */
function settled(
  state: EditorState,
  intent: DiagramIntent,
): DiagramIntent | null {
  const fence = state
    .field(diagramField)
    .fences.find((f) => f.from <= intent.anchor && intent.anchor <= f.to);
  if (fence === undefined) {
    return null;
  }
  if (intent.mode !== "focus" && !writable(state)) {
    return { anchor: fence.from, target: intent.target, mode: "focus" };
  }
  return intent.anchor === fence.from
    ? intent
    : { ...intent, anchor: fence.from };
}

export const diagramIntent = StateField.define<DiagramIntent | null>({
  create: () => null,
  update: (value, tr) => {
    let next = value;
    if (next !== null && tr.docChanged) {
      // Not plain mapping: a deleted fence's anchor would land on the next one.
      const anchor = tr.changes.mapPos(next.anchor, 1, MapMode.TrackAfter);
      next = anchor === null ? null : { ...next, anchor };
    }
    for (const effect of tr.effects) {
      if (effect.is(setDiagramIntent)) {
        next = effect.value;
      }
    }
    return next === null ? null : settled(tr.state, next);
  },
});

/** What was open, outside the field; absent when nothing is open. */
interface OpenRecord {
  intent: DiagramIntent;
  draft?: string;
}

const opens = new WeakMap<EditorView, OpenRecord>();

type CloseListener = (reason: IntentCloseReason) => void;
type BeforeClose = (reason: IntentCloseReason, intent: DiagramIntent) => void;

const closeListeners = new WeakMap<EditorView, Set<CloseListener>>();
const beforeCloseHooks = new WeakMap<EditorView, Set<BeforeClose>>();

function add<T>(
  registry: WeakMap<EditorView, Set<T>>,
  main: EditorView,
  item: T,
) {
  const set = registry.get(main) ?? new Set<T>();
  registry.set(main, set);
  set.add(item);
  return () => {
    set.delete(item);
  };
}

/** Told after the intent closed, once per close. Keeps working through `setState`. */
export function onIntentClosed(
  main: EditorView,
  listener: CloseListener,
): () => void {
  return add(closeListeners, main, listener);
}

/**
 * Runs before a close, with the intent as it stood and its latest draft. On
 * "raw" the field is already gone: Task 11 commits a rename draft from here, and
 * the apply layer it calls reads the document and `diagramAnchors`, not the field.
 */
export function beforeClose(main: EditorView, hook: BeforeClose): () => void {
  return add(beforeCloseHooks, main, hook);
}

/** Keeps the draft for `beforeClose` current, on every input of the open field. */
export function noteIntentDraft(main: EditorView, draft: string): void {
  const open = opens.get(main);
  if (open !== undefined) {
    open.draft = draft;
  }
}

/** How a mode comes back when its fence is mounted again. */
type Reopen = (
  main: EditorView,
  intent: DiagramIntent,
  host: DiagramHost,
) => void;

const reopeners = new Map<IntentMode, Reopen>();

/**
 * What a remount does for `mode`, instead of focusing the canvas: Tasks 10-12
 * re-resolve the target's element and reopen their field or menu here.
 */
export function registerIntentReopen(mode: IntentMode, reopen: Reopen): void {
  reopeners.set(mode, reopen);
}

function destroyed(view: EditorView): boolean {
  return (view as unknown as { destroyed?: unknown }).destroyed === true;
}

/** Whether the focus sits somewhere else that matters, outside this editor. */
function focusIsElsewhere(main: EditorView): boolean {
  const active = main.root.activeElement;
  return (
    active !== null &&
    active !== main.dom.ownerDocument.body &&
    !main.dom.contains(active)
  );
}

/** Moves the caret to the line after the fence, when a fence is still there. */
function caretAfterFence(main: EditorView, anchor: number): void {
  const { state } = main;
  const fence = anchor <= state.doc.length ? fenceAt(state, anchor) : null;
  if (fence === null) {
    return;
  }
  const line = state.doc.lineAt(fence.to);
  const after =
    line.number < state.doc.lines
      ? state.doc.line(line.number + 1).from
      : line.to;
  main.dispatch({
    selection: EditorSelection.cursor(after),
    scrollIntoView: true,
    userEvent: "select",
  });
}

function placeFocus(
  main: EditorView,
  reason: IntentCloseReason,
  anchor: number,
): void {
  // A fence a peer removed does not pull the focus out of another field.
  if (reason === "gone" && focusIsElsewhere(main)) {
    return;
  }
  // After a swap the anchor names a place in another document, and after "gone"
  // no fence is left to stand after.
  if (reason === "user" || reason === "raw") {
    caretAfterFence(main, anchor);
  }
  main.focus();
}

/**
 * The one teardown path. Does nothing when no intent is open, or, for "gone",
 * when the intent's own fence is still drawn (the verdict watcher reports any
 * fence it lost). Never inside an update.
 */
export function closeDiagramIntent(
  main: EditorView,
  reason: IntentCloseReason,
  focusTo: IntentFocus = "main-after",
): void {
  const open = opens.get(main);
  if (open === undefined) {
    return;
  }
  const alive = main.state.field(diagramIntent, false) ?? null;
  if (reason === "gone" && alive !== null) {
    return;
  }
  opens.delete(main);
  const intent =
    open.draft === undefined
      ? open.intent
      : { ...open.intent, draft: open.draft };
  for (const hook of beforeCloseHooks.get(main) ?? []) {
    hook(reason, intent);
  }
  if (destroyed(main)) {
    for (const listener of closeListeners.get(main) ?? []) {
      listener(reason);
    }
    return;
  }
  if (main.state.field(diagramIntent, false)) {
    main.dispatch({ effects: setDiagramIntent.of(null) });
  }
  for (const listener of closeListeners.get(main) ?? []) {
    listener(reason);
  }
  if (focusTo === "main-after" && reason !== "destroy") {
    placeFocus(main, reason, intent.anchor);
  }
}

/** A fence back in the viewport: its intent comes back with it. */
function restore(main: EditorView, host: DiagramHost): void {
  const intent = main.state.field(diagramIntent, false);
  if (!intent || hostOf(main, intent.anchor) !== host) {
    return;
  }
  const reopen = reopeners.get(intent.mode);
  if (reopen !== undefined) {
    reopen(main, intent, host);
    return;
  }
  // Until a later task knows the target's element: the canvas. The focus went
  // when the old box left, so only an unclaimed focus comes back here.
  const active = main.root.activeElement;
  if (active === null || active === main.dom.ownerDocument.body) {
    host.container.focus({ preventScroll: true });
  }
}

function sameTarget(a: DiagramIntent, b: DiagramIntent): boolean {
  return (
    a.mode === b.mode && JSON.stringify(a.target) === JSON.stringify(b.target)
  );
}

/** Keeps the record beside the field and notices what takes the field away. */
export const intentWatcher = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;
    readonly unmount: () => void;

    constructor(view: EditorView) {
      this.view = view;
      this.unmount = onBoxMounted(view, (host) => {
        restore(view, host);
      });
    }

    update(update: ViewUpdate): void {
      const next = update.state.field(diagramIntent);
      const open = opens.get(this.view);
      if (next !== null) {
        const draft =
          next.draft ??
          (open !== undefined && sameTarget(open.intent, next)
            ? open.draft
            : undefined);
        opens.set(
          this.view,
          draft === undefined ? { intent: next } : { intent: next, draft },
        );
      } else if (open !== undefined) {
        queueMicrotask(() => {
          closeDiagramIntent(this.view, "gone");
        });
      }
    }

    /**
     * Three things end this plugin. A reconfigure has already put the new state
     * in place when it gets here: no field means Raw. `setState` and a destroy
     * still show the old state, and the next microtask tells them apart.
     */
    destroy(): void {
      this.unmount();
      const { view } = this;
      if (!opens.has(view)) {
        return;
      }
      if (view.state.field(diagramIntent, false) === undefined) {
        queueMicrotask(() => {
          closeDiagramIntent(view, "raw", "main-after");
        });
        return;
      }
      const before = view.state;
      queueMicrotask(() => {
        if (destroyed(view)) {
          closeDiagramIntent(view, "destroy", "none");
        } else if (view.state !== before) {
          closeDiagramIntent(view, "swap", "main-after");
        }
      });
    }
  },
);
