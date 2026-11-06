/**
 * The keyboard and the pointer on a drawn diagram. A diagram is one composite
 * widget with a roving tabindex (WAI-ARIA): one Tab stop, the arrow keys walk
 * canvas, boxes and arrows in source order, Home and End jump, Escape leaves.
 *
 * The listeners sit on the content element, not in CodeMirror's handlers: the
 * widget's `ignoreEvent` keeps every event in a box away from those, which is
 * also what keeps a key on the drawing from editing the text.
 *
 * Every action first asks whether the drawing on screen is the text as it stands
 * (`staleNow`), and answers "stale" and does nothing when it is not. Escape and
 * undo do not ask: neither reads the drawing, and no state may trap the keyboard.
 *
 * `fenceMarksGuard` is the net under the keys: no person's edit may leave a drawn
 * fence's marks broken, whichever command made it.
 */

import { forceParsing, syntaxTree } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorState, Prec } from "@codemirror/state";
import type { Command, ViewUpdate } from "@codemirror/view";
import {
  EditorView,
  ViewPlugin,
  keymap,
  runScopeHandlers,
} from "@codemirror/view";

import { stepVisible, viewAtoms } from "../marks/caret";
import { flowKind, removeFence, runDiagramOp } from "./apply";
import { drawnFenceLines, stepIntoDrawnFence } from "./draws";
import type { RefusalCode } from "./edits";
import { fenceAt } from "./fence";
import { diagramField, fenceIsBehind, verdictArrived } from "./field";
import {
  decorateFlow,
  flowElementOf,
  flowStops,
  flowTargetOf,
} from "./flowchart/hit";
import type { FlowModel } from "./flowchart/model";
import { parseFlowchart } from "./flowchart/parse";
import type { DiagramIntent, TargetKey } from "./intent";
import {
  closeDiagramIntent,
  diagramIntent,
  registerIntentReopen,
  setDiagramIntent,
} from "./intent";
import type { Verdict } from "./verdict";
import { verdictFor } from "./verdict";
import type { DiagramHost } from "./widget";
import { hostOf, onBoxMounted, onDiagramDrawn } from "./widget";

const CANVAS: TargetKey = { kind: "canvas" };
const FOCUS_CLASS = "cm-diagram-focus";
const RING_CLASS = "cm-diagram-focus-ring";
/** How long a stale answer may wait for the parse to reach the fence. */
const PARSE_MS = 50;
/** The space around a stop's box, so a straight arrow's zero width still shows. */
const RING_PADDING = 3;
const MODELS_KEPT = 8;

type StopAction = (
  main: EditorView,
  anchor: number,
  target: TargetKey,
  host: DiagramHost,
) => void;

const stopActions = new Map<"rename" | "menu", StopAction>();

/** What Enter or F2 (rename) and Shift+F10 (menu) do: Tasks 11 and 12 fill these. */
export function registerStopAction(
  name: "rename" | "menu",
  action: StopAction,
): void {
  stopActions.set(name, action);
}

const models = new Map<string, FlowModel>();

function modelOf(body: string): FlowModel {
  let model = models.get(body);
  if (model === undefined) {
    model = parseFlowchart(body);
    models.set(body, model);
    if (models.size > MODELS_KEPT) {
      const oldest = models.keys().next().value;
      if (oldest !== undefined) {
        models.delete(oldest);
      }
    }
  }
  return model;
}

interface Reading {
  svg: SVGSVGElement;
  svgId: string;
  model: FlowModel;
  verdict: Verdict;
}

/** The drawing on screen with its own body's model; null until both are known. */
function readingOf(host: DiagramHost): Reading | null {
  const svg = host.svg();
  const body = host.renderedBody();
  const verdict = verdictFor(body);
  if (svg === null || verdict === undefined || !verdict.ok) {
    return null;
  }
  const model = modelOf(body);
  return model.understood ? { svg, svgId: host.svgId(), model, verdict } : null;
}

/** The stops that have an element on screen; only those can take the focus. */
function stopsOf(reading: Reading | null): TargetKey[] {
  if (reading === null) {
    return [CANVAS];
  }
  return flowStops(reading.model).filter(
    (stop) => stop.kind === "canvas" || elementOf(reading, stop) !== null,
  );
}

function elementOf(reading: Reading, target: TargetKey) {
  return flowElementOf(
    reading.svg,
    reading.svgId,
    target,
    reading.model,
    reading.verdict,
  );
}

const sameTarget = (a: TargetKey, b: TargetKey) =>
  JSON.stringify(a) === JSON.stringify(b);

function stopElement(
  host: DiagramHost,
  target: TargetKey,
): HTMLElement | SVGElement {
  const reading = readingOf(host);
  return (reading && elementOf(reading, target)) ?? host.container;
}

/** Takes the look off the box, and puts it on `el` when one is given. */
function paint(host: DiagramHost, el: HTMLElement | SVGElement | null): void {
  const { container, overlay } = host;
  container.classList.remove(FOCUS_CLASS);
  for (const old of container.querySelectorAll(`.${FOCUS_CLASS}`)) {
    old.classList.remove(FOCUS_CLASS);
  }
  overlay.querySelector(`:scope > .${RING_CLASS}`)?.remove();
  if (el === null) {
    return;
  }
  el.classList.add(FOCUS_CLASS);
  const ring = document.createElement("div");
  ring.className = RING_CLASS;
  ring.setAttribute("aria-hidden", "true");
  overlay.append(ring);
  placeRing(ring, el, overlay);
}

function placeRing(ring: HTMLElement, el: Element, overlay: HTMLElement): void {
  const box = el.getBoundingClientRect();
  const frame = overlay.getBoundingClientRect();
  ring.style.left = `${String(box.left - frame.left - RING_PADDING)}px`;
  ring.style.top = `${String(box.top - frame.top - RING_PADDING)}px`;
  ring.style.width = `${String(box.width + 2 * RING_PADDING)}px`;
  ring.style.height = `${String(box.height + 2 * RING_PADDING)}px`;
}

/** Gives `target`'s element the one `tabindex="0"` of this box. */
function rove(host: DiagramHost, target: TargetKey) {
  const el = stopElement(host, target);
  const { container } = host;
  for (const old of container.querySelectorAll('[tabindex="0"]')) {
    old.setAttribute("tabindex", "-1");
  }
  container.setAttribute("tabindex", el === container ? "0" : "-1");
  el.setAttribute("tabindex", "0");
  return el;
}

/** Where the last position in the stops was, for a stop that goes away. */
const lastPlace = new WeakMap<HTMLElement, number>();

/** Fences to focus when they mount: asked for while they were off screen. */
const wanted = new WeakSet<EditorView>();

/**
 * Puts the keyboard on a stop of the fence at `anchor`, any position in it: the
 * intent holds it in "focus" mode, so it comes back with a remount. A fence off
 * screen is scrolled to, and takes the focus when its box arrives.
 */
export function focusTarget(
  main: EditorView,
  anchor: number,
  target: TargetKey,
): void {
  const host = hostOf(main, anchor);
  if (host === null) {
    wanted.add(main);
    main.dispatch({
      effects: [
        setDiagramIntent.of({ anchor, target, mode: "focus" }),
        EditorView.scrollIntoView(anchor, { y: "center" }),
      ],
    });
    return;
  }
  main.dispatch({
    effects: setDiagramIntent.of({ anchor, target, mode: "focus" }),
  });
  const place = stopsOf(readingOf(host)).findIndex((stop) =>
    sameTarget(stop, target),
  );
  lastPlace.set(host.container, Math.max(place, 0));
  const el = rove(host, target);
  paint(host, el);
  el.focus();
}

/** The chunk's side of the hand-off from the hidden-mark keys (R13). */
export function enterCanvas(main: EditorView, pos: number): void {
  focusTarget(main, pos, CANVAS);
}

registerIntentReopen("focus", (main, intent, host) => {
  const asked = wanted.delete(main);
  const active = main.root.activeElement;
  const el = rove(host, intent.target);
  // The focus went when the old box left, so only an unclaimed one comes back.
  if (asked || active === null || active === main.dom.ownerDocument.body) {
    paint(host, el);
    el.focus({ preventScroll: true });
  }
});

/** The drawn fence whose box is `container`, by the line the box stands for. */
function fenceFromOf(main: EditorView, container: HTMLElement): number | null {
  const start = main.posAtDOM(container);
  const fence = main.state
    .field(diagramField)
    .fences.find((f) => main.state.doc.lineAt(f.from).from === start);
  return fence === undefined ? null : fence.from;
}

/**
 * Whether the drawing is behind the text. A carried fence's `body` is the old
 * one, which equals the drawing, so the text is read again from a tree that
 * reaches it; forcing that parse is also what keeps a fence from staying behind
 * while the worker has no budget, with the focus off the main view (R79 N1).
 * `intentStale` asks the same of the open intent; a first click has none.
 */
function staleNow(main: EditorView, host: DiagramHost, from: number): boolean {
  const behind = fenceIsBehind(main.state, from);
  const drawn = main.state
    .field(diagramField)
    .fences.find((f) => f.from === from);
  if (behind || (drawn && syntaxTree(main.state).length < drawn.to)) {
    forceParsing(main, drawn?.to ?? from, PARSE_MS);
  }
  const body = fenceAt(main.state, from)?.body;
  if (behind || body === undefined || body !== host.renderedBody()) {
    host.notice("stale");
    return true;
  }
  return false;
}

async function deleteStop(
  main: EditorView,
  host: DiagramHost,
  from: number,
  target: TargetKey,
): Promise<void> {
  let refused: RefusalCode | undefined;
  if (target.kind === "canvas") {
    const outcome = removeFence(main, from);
    refused = "refused" in outcome ? outcome.refused : undefined;
  } else {
    const op =
      target.kind === "node"
        ? ({ op: "deleteNode", id: target.id } as const)
        : ({ op: "deleteEdge", edge: target.key } as const);
    const outcome = await runDiagramOp(main, from, flowKind, op);
    refused = "refused" in outcome ? outcome.refused : undefined;
  }
  if (refused !== undefined) {
    host.notice(refused);
  }
}

/** The stop a key walks from: the one the intent holds, when it is this fence's. */
function currentTarget(main: EditorView, from: number): TargetKey {
  const intent = main.state.field(diagramIntent);
  return intent !== null && intent.anchor === from ? intent.target : CANVAS;
}

function walk(
  main: EditorView,
  host: DiagramHost,
  from: number,
  key: string,
): void {
  const stops = stopsOf(readingOf(host));
  const at = stops.findIndex((stop) =>
    sameTarget(stop, currentTarget(main, from)),
  );
  const here = Math.max(at, 0);
  const next =
    key === "Home"
      ? stops.findIndex((stop) => stop.kind !== "canvas")
      : key === "End"
        ? stops.length - 1
        : key === "ArrowRight" || key === "ArrowDown"
          ? Math.min(here + 1, stops.length - 1)
          : Math.max(here - 1, 0);
  // Home with no box stays where it is.
  const target = stops[next < 0 ? here : next] ?? CANVAS;
  focusTarget(main, from, target);
}

const WALK_KEYS = new Set([
  "ArrowRight",
  "ArrowDown",
  "ArrowLeft",
  "ArrowUp",
  "Home",
  "End",
]);

function isUndoKey(event: KeyboardEvent): boolean {
  return (event.metaKey || event.ctrlKey) && /^[zy]$/i.test(event.key);
}

function leave(
  main: EditorView,
  host: DiagramHost,
  intent: DiagramIntent | null,
) {
  if (intent !== null && intent.mode !== "focus") {
    main.dispatch({
      effects: setDiagramIntent.of({
        anchor: intent.anchor,
        target: intent.target,
        mode: "focus",
      }),
    });
    stopElement(host, intent.target).focus();
    return;
  }
  closeDiagramIntent(main, "user");
}

/** Answers a key on the box; false when it is not one of the diagram's. */
function onKey(
  main: EditorView,
  host: DiagramHost,
  from: number,
  event: KeyboardEvent,
): boolean {
  if (isUndoKey(event)) {
    // The drawing holds the focus, and the undo stack is the page's.
    return runScopeHandlers(main, event, "editor");
  }
  if (event.ctrlKey || event.metaKey || event.altKey) {
    return false;
  }
  const { key } = event;
  const menu = key === "ContextMenu" || (key === "F10" && event.shiftKey);
  if (event.shiftKey && !menu) {
    return false;
  }
  if (key === "Escape") {
    leave(main, host, main.state.field(diagramIntent));
    return true;
  }
  const rename = key === "Enter" || key === "F2";
  const remove = key === "Delete" || key === "Backspace";
  if (!menu && !rename && !remove && !WALK_KEYS.has(key)) {
    return false;
  }
  if (staleNow(main, host, from)) {
    return true;
  }
  const target = currentTarget(main, from);
  if (WALK_KEYS.has(key)) {
    walk(main, host, from, key);
  } else if (remove) {
    if (host.writable()) {
      void deleteStop(main, host, from, target);
    }
  } else if (menu) {
    stopActions.get("menu")?.(main, from, target, host);
  } else if (target.kind !== "canvas") {
    stopActions.get("rename")?.(main, from, target, host);
  }
  return true;
}

/** The box an event happened in, unless it happened in a field of its overlay. */
function boxOf(event: Event): HTMLElement | null {
  const el = event.target;
  if (!(el instanceof Element) || el.closest(".cm-diagram-overlay") !== null) {
    return null;
  }
  return el.closest<HTMLElement>(".cm-diagram");
}

/** Decorates a new drawing and moves what the intent holds onto its elements. */
function settle(main: EditorView, host: DiagramHost): void {
  const reading = readingOf(host);
  if (reading !== null && !decorated.has(reading.svg)) {
    decorated.add(reading.svg);
    decorateFlow(reading.svg, reading.svgId, reading.model, reading.verdict);
  }
  const intent = main.state.field(diagramIntent);
  if (intent === null || hostOf(main, intent.anchor) !== host) {
    rove(host, CANVAS);
    return;
  }
  if (intent.mode !== "focus") {
    return;
  }
  let { target } = intent;
  const stops = stopsOf(reading);
  if (!stops.some((stop) => sameTarget(stop, target))) {
    // The stop went, by a delete or a peer: the one now in its place, never the
    // body, and only a drawing that can be read says it went.
    if (reading === null) {
      return;
    }
    const place = lastPlace.get(host.container) ?? 0;
    target = stops[Math.min(place, stops.length - 1)] ?? CANVAS;
    main.dispatch({
      effects: setDiagramIntent.of({ ...intent, target }),
    });
  }
  const active = main.root.activeElement;
  const lost =
    active === null ||
    active === main.dom.ownerDocument.body ||
    host.container.contains(active);
  const el = rove(host, target);
  if (lost) {
    paint(host, el);
    el.focus({ preventScroll: true });
  }
}

const decorated = new WeakSet<SVGSVGElement>();

/** The stop whose source spans hold `pos`; the canvas otherwise. */
function stopAtPos(host: DiagramHost, offset: number): TargetKey {
  const reading = readingOf(host);
  if (reading === null) {
    return CANVAS;
  }
  const { model } = reading;
  for (const id of model.nodeOrder) {
    const refs = model.nodes.get(id)?.refs ?? [];
    if (refs.some((r) => r.span.from <= offset && offset <= r.span.to)) {
      return { kind: "node", id };
    }
  }
  for (const edge of model.edges) {
    const gap = model.chains[edge.chain]?.links[edge.link]?.gap;
    if (gap !== undefined && gap.from <= offset && offset <= gap.to) {
      return {
        kind: "edge",
        key: { start: edge.start, end: edge.end, occurrence: edge.occurrence },
      };
    }
  }
  return CANVAS;
}

/** Keeps the keyboard's places and the look current, and takes the DOM events. */
export const diagramFocus: Extension = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;
    readonly stop: (() => void)[];
    /** The box wearing a search hit's look. */
    lookOn: DiagramHost | null = null;

    readonly mousedown = (event: Event): void => {
      // The click decides; a press alone moves neither focus nor selection.
      if (boxOf(event) !== null) {
        event.preventDefault();
      }
    };
    /**
     * A press in the text ends what the drawing held, before CodeMirror's own
     * handler sets the caret: an open intent parks main's selection, and a parked
     * caret is not brought to its canonical place.
     */
    readonly pressOutside = (event: Event): void => {
      const el = event.target;
      if (
        el instanceof Element &&
        el.closest(".cm-diagram") === null &&
        this.view.state.field(diagramIntent) !== null
      ) {
        closeDiagramIntent(this.view, "user", "none");
      }
    };
    readonly click = (event: Event): void => {
      const place = this.place(event);
      if (place === null || staleNow(this.view, place.host, place.from)) {
        return;
      }
      const reading = readingOf(place.host);
      const el = event.target as Element;
      focusTarget(
        this.view,
        place.from,
        reading === null
          ? CANVAS
          : flowTargetOf(el, reading.svgId, reading.model, reading.verdict),
      );
    };
    readonly contextmenu = (event: Event): void => {
      const place = this.place(event);
      if (place === null) {
        return;
      }
      if (staleNow(this.view, place.host, place.from)) {
        event.preventDefault();
        return;
      }
      const action = stopActions.get("menu");
      if (action !== undefined) {
        event.preventDefault();
        const reading = readingOf(place.host);
        action(
          this.view,
          place.from,
          reading === null
            ? CANVAS
            : flowTargetOf(
                event.target as Element,
                reading.svgId,
                reading.model,
                reading.verdict,
              ),
          place.host,
        );
      }
    };
    readonly keydown = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || event.isComposing) {
        return;
      }
      const place = this.place(event);
      if (place !== null && onKey(this.view, place.host, place.from, event)) {
        event.preventDefault();
      }
    };
    readonly focusIn = (event: Event): void => {
      const box = boxOf(event);
      if (box === null) {
        return;
      }
      const el = event.target as Element;
      queueMicrotask(() => {
        this.focused(box, el);
      });
    };
    readonly focusOut = (event: FocusEvent): void => {
      const box = boxOf(event);
      // No next element: a window blur, or the box scrolling away. The intent
      // stays for the way back.
      if (box === null || event.relatedTarget === null) {
        return;
      }
      if (!box.contains(event.relatedTarget as Node)) {
        queueMicrotask(() => {
          this.left(box);
        });
      }
    };

    constructor(view: EditorView) {
      this.view = view;
      const dom = view.contentDOM;
      dom.addEventListener("mousedown", this.mousedown);
      view.dom.addEventListener("mousedown", this.pressOutside, true);
      dom.addEventListener("click", this.click);
      dom.addEventListener("contextmenu", this.contextmenu);
      dom.addEventListener("keydown", this.keydown);
      dom.addEventListener("focus", this.focusIn, true);
      dom.addEventListener("blur", this.focusOut, true);
      this.stop = [
        onDiagramDrawn(view, (host) => {
          settle(view, host);
        }),
        onBoxMounted(view, (host) => {
          settle(view, host);
        }),
      ];
    }

    place(event: Event): { host: DiagramHost; from: number } | null {
      const box = boxOf(event);
      const from = box === null ? null : fenceFromOf(this.view, box);
      const host = from === null ? null : hostOf(this.view, from);
      return host === null || from === null ? null : { host, from };
    }

    /** The focus came to a stop: by Tab, or a browser giving it back. */
    focused(box: HTMLElement, el: Element): void {
      const { view } = this;
      const from = box.isConnected ? fenceFromOf(view, box) : null;
      const host = from === null ? null : hostOf(view, from);
      if (host === null || from === null || !el.isConnected) {
        return;
      }
      const reading = readingOf(host);
      const target =
        reading === null || el === box
          ? CANVAS
          : flowTargetOf(el, reading.svgId, reading.model, reading.verdict);
      const intent = view.state.field(diagramIntent);
      if (
        intent === null ||
        intent.anchor !== from ||
        !sameTarget(intent.target, target)
      ) {
        view.dispatch({
          effects: setDiagramIntent.of({ anchor: from, target, mode: "focus" }),
        });
      }
      paint(host, rove(host, target));
    }

    /** The focus went elsewhere in the page: nothing is held any more. */
    left(box: HTMLElement): void {
      const { view } = this;
      const intent = view.state.field(diagramIntent, false);
      if (!intent) {
        this.clear(box);
        return;
      }
      const host = hostOf(view, intent.anchor);
      const active = view.root.activeElement;
      if (host !== null && active !== null && host.container.contains(active)) {
        return;
      }
      closeDiagramIntent(view, "user", "none");
      this.clear(box);
    }

    clear(box: HTMLElement): void {
      const from = box.isConnected ? fenceFromOf(this.view, box) : null;
      const host = from === null ? null : hostOf(this.view, from);
      if (host !== null) {
        paint(host, null);
      }
    }

    update(update: ViewUpdate): void {
      const { view } = this;
      if (
        update.transactions.some((tr) =>
          tr.effects.some((e) => e.is(verdictArrived)),
        )
      ) {
        queueMicrotask(() => {
          this.redecorate();
        });
      }
      if (!update.selectionSet) {
        return;
      }
      const { from, to } = update.state.selection.main;
      const fence = update.state
        .facet(drawnFenceLines)
        .find((f) => f.from <= from && to <= f.to);
      const search = update.transactions.some((tr) =>
        tr.isUserEvent("select.search"),
      );
      queueMicrotask(() => {
        // Another selection or text since: that update decides. Anything else,
        // such as a close dispatched in this tick, changes nothing here.
        if (
          view.state.doc !== update.state.doc ||
          !view.state.selection.eq(update.state.selection)
        ) {
          return;
        }
        this.clearLook();
        if (fence === undefined) {
          return;
        }
        if (search) {
          this.showHit(from);
        } else if (view.hasFocus && !view.composing) {
          focusTarget(view, from, CANVAS);
        }
      });
    }

    redecorate(): void {
      const { view } = this;
      for (const fence of view.state.field(diagramField).fences) {
        const host = hostOf(view, fence.from);
        if (host !== null) {
          settle(view, host);
        }
      }
    }

    clearLook(): void {
      if (this.lookOn !== null) {
        const intent = this.view.state.field(diagramIntent);
        if (intent === null) {
          paint(this.lookOn, null);
        }
        this.lookOn = null;
      }
    }

    /** A search hit shows on its stop, and the focus stays in the search. */
    showHit(head: number): void {
      const host = hostOf(this.view, head);
      const fence = fenceAt(this.view.state, head);
      if (host === null || fence === null) {
        return;
      }
      const current = host.renderedBody() === fence.body;
      paint(
        host,
        stopElement(
          host,
          current ? stopAtPos(host, head - fence.bodyFrom) : CANVAS,
        ),
      );
      this.lookOn = host;
    }

    destroy(): void {
      const dom = this.view.contentDOM;
      dom.removeEventListener("mousedown", this.mousedown);
      this.view.dom.removeEventListener("mousedown", this.pressOutside, true);
      dom.removeEventListener("click", this.click);
      dom.removeEventListener("contextmenu", this.contextmenu);
      dom.removeEventListener("keydown", this.keydown);
      dom.removeEventListener("focus", this.focusIn, true);
      dom.removeEventListener("blur", this.focusOut, true);
      for (const stop of this.stop) {
        stop();
      }
    }
  },
);

/** An empty caret at the visible edge of its line, or null. */
function edgeOf(view: EditorView, forward: boolean): number | null {
  const { state } = view;
  const { selection } = state;
  if (selection.ranges.length > 1 || !selection.main.empty) {
    return null;
  }
  const { head } = selection.main;
  if (stepVisible(state, head, forward, viewAtoms(view)) !== null) {
    return null;
  }
  const line = state.doc.lineAt(head);
  return forward ? line.to : line.from;
}

function sideways(forward: boolean): Command {
  return (view) => {
    const edge = edgeOf(view, forward);
    return edge !== null && stepIntoDrawnFence(view, edge, forward);
  };
}

function vertical(forward: boolean): Command {
  return (view) => {
    const { state } = view;
    const { selection } = state;
    if (selection.ranges.length > 1 || !selection.main.empty) {
      return false;
    }
    const range = selection.main;
    const line = state.doc.lineAt(range.head);
    // A wrapped line has rows of its own to move through first.
    const moved = view.moveVertically(range, forward);
    if (
      moved.head !== range.head &&
      state.doc.lineAt(moved.head).number === line.number
    ) {
      return false;
    }
    return stepIntoDrawnFence(view, forward ? line.to : line.from, forward);
  };
}

function deleting(forward: boolean): Command {
  return (view) => !view.state.readOnly && sideways(forward)(view);
}

/** The main view's ways onto a drawn fence's canvas. */
export const diagramEntryKeys: Extension = Prec.high(
  keymap.of([
    { key: "ArrowDown", run: vertical(true) },
    { key: "ArrowUp", run: vertical(false) },
    { key: "ArrowRight", run: sideways(true) },
    { key: "ArrowLeft", run: sideways(false) },
    { key: "Backspace", run: deleting(false) },
    { key: "Delete", run: deleting(true) },
    { key: "Mod-Backspace", mac: "Alt-Backspace", run: deleting(false) },
    { key: "Mod-Delete", mac: "Alt-Delete", run: deleting(true) },
    { mac: "Mod-Backspace", run: deleting(false) },
    { mac: "Mod-Delete", run: deleting(true) },
  ]),
);

/**
 * Refuses a person's edit that would break a drawn fence's marks: a line joined
 * to one, a line moved past one (R73), text glued on by a selection that ends
 * at one. An edit inside the body or across the whole fence passes, and so do a
 * peer's edit and an undo, which carry no such event. A refusal beats a wrong edit.
 */
export const fenceMarksGuard: Extension = EditorState.transactionFilter.of(
  (tr) => {
    if (
      !tr.docChanged ||
      !(
        tr.isUserEvent("input") ||
        tr.isUserEvent("delete") ||
        tr.isUserEvent("move")
      ) ||
      tr.isUserEvent("input.diagram") ||
      tr.isUserEvent("delete.diagram")
    ) {
      return tr;
    }
    const { doc } = tr.startState;
    for (const fence of tr.startState.facet(drawnFenceLines)) {
      if (tr.changes.touchesRange(fence.from, fence.to) === false) {
        continue;
      }
      const from = tr.changes.mapPos(fence.from, 1);
      const to = tr.changes.mapPos(fence.to, -1);
      if (from >= to) {
        continue;
      }
      const first = tr.newDoc.lineAt(from);
      const last = tr.newDoc.lineAt(to);
      if (
        first.from !== from ||
        last.to !== to ||
        first.text !== doc.lineAt(fence.from).text ||
        last.text !== doc.lineAt(fence.to).text
      ) {
        return [];
      }
    }
    return tr;
  },
);
