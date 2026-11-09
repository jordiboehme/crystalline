/**
 * The keyboard and the pointer on a drawn diagram. A diagram is one composite
 * widget with a roving tabindex (WAI-ARIA): one Tab stop, the arrow keys walk
 * canvas, boxes and arrows in source order, Home and End jump, Escape leaves.
 * Shift+F10 or a right click opens a stop's menu, and while connect mode waits its
 * keys walk the boxes alone and Enter or a click chooses (`flowchart/actions.ts`).
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
import {
  EditorSelection,
  EditorState,
  Prec,
  StateEffect,
  StateField,
  Transaction,
} from "@codemirror/state";
import type { Command, ViewUpdate } from "@codemirror/view";
import {
  EditorView,
  ViewPlugin,
  keymap,
  runScopeHandlers,
  showPanel,
} from "@codemirror/view";

import { isMac, isMenuKey } from "../../platform";
import { writable } from "../elementEdit";
import { stepVisible, viewAtoms } from "../marks/caret";
import { dispatchIsolated, roomUndo } from "../roomUndo";
import { flowKind, removeFence, runDiagramOp, seqKind } from "./apply";
import { drawnFenceLines, stepIntoDrawnFence } from "./draws";
import type { RefusalCode } from "./edits";
import { NOTICES } from "./edits";
import { fenceAt } from "./fence";
import {
  diagramField,
  fenceIsBehind,
  showFenceSource,
  verdictArrived,
} from "./field";
import type { FlowOp } from "./flowchart/ops";
import type { LineHold } from "./held";
import {
  followLine,
  heldMessageOp,
  heldTargetOf,
  holdLine,
  letGoOfLine,
  resolveHeldTarget,
} from "./held";
import type { DiagramIntent, TargetKey } from "./intent";
import {
  closeDiagramIntent,
  diagramIntent,
  registerIntentReopen,
  setDiagramIntent,
} from "./intent";
import type { DiagramModel } from "./kinds";
import {
  connectIdOf,
  connectable,
  decorateDrawing,
  elementOfTarget,
  isLine,
  labelOfTarget,
  parseDiagram,
  startOf,
  stopAtOffset,
  stopsOf as kindStops,
  targetOfElement,
} from "./kinds";
import { heldSeqOp } from "./sequence/ops";
import type { Verdict } from "./verdict";
import { verdictFor } from "./verdict";
import type { DiagramHost } from "./widget";
import { hostOf, NOTICE_MS, onBoxMounted, onDiagramDrawn } from "./widget";

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
  /** Where a pointer asked, in the window's coordinates. */
  at?: { x: number; y: number },
) => void;

const stopActions = new Map<"rename" | "menu", StopAction>();

/** What Enter or F2 (rename) and Shift+F10 (menu) do: Tasks 11 and 12 fill these. */
export function registerStopAction(
  name: "rename" | "menu",
  action: StopAction,
): void {
  stopActions.set(name, action);
}

type ClickGate = (
  main: EditorView,
  target: TargetKey,
  go: () => void,
  stay: () => void,
) => void;

let clickGate: ClickGate | null = null;

/**
 * What stands before a click on a stop: an open rename field writes its draft
 * first, and `go` then takes the click. If the field stays open it calls `stay`.
 */
export function registerClickGate(gate: ClickGate): void {
  clickGate = gate;
}

/** What a key or a click does while connect mode waits for its box. */
export interface ConnectHooks {
  choose(main: EditorView, id: string): void;
  cancel(main: EditorView): void;
  /** A click on an arrow: no box, so the mode says again what it wants. */
  remind(main: EditorView): void;
}

let connectHooks: ConnectHooks | null = null;

export function registerConnectHooks(hooks: ConnectHooks): void {
  connectHooks = hooks;
}

const models = new Map<string, DiagramModel>();

/** The model of a fence body, of its kind, kept for the last few bodies. */
export function modelOf(body: string): DiagramModel {
  let model = models.get(body);
  if (model === undefined) {
    model = parseDiagram(body);
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
  model: DiagramModel;
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
  return kindStops(reading.model).filter(
    (stop) => stop.kind === "canvas" || elementOf(reading, stop) !== null,
  );
}

function elementOf(reading: Reading, target: TargetKey) {
  return elementOfTarget(
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

/** The element `target` has in the drawing on screen, or null. */
export function targetElement(
  host: DiagramHost,
  target: TargetKey,
): HTMLElement | SVGElement | null {
  const reading = readingOf(host);
  return reading && elementOf(reading, target);
}

/** The look of the focus off the box: the field stands where the ring would. */
export function clearStopLook(host: DiagramHost): void {
  paint(host, null);
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
 * intent holds it, in "focus" mode unless connect mode is walking, so it comes back
 * with a remount. A fence off screen is scrolled to, and takes the focus when its
 * box arrives.
 */
export function focusTarget(
  main: EditorView,
  anchor: number,
  target: TargetKey,
  mode: "focus" | "connect" = "focus",
): void {
  const host = hostOf(main, anchor);
  if (host === null) {
    wanted.add(main);
    main.dispatch({
      effects: [
        setDiagramIntent.of({ anchor, target, mode }),
        EditorView.scrollIntoView(anchor, { y: "center" }),
      ],
    });
    return;
  }
  main.dispatch({
    effects: setDiagramIntent.of({ anchor, target, mode }),
  });
  prepareStop(host, target).focus();
}

/** Gives `target` the roving tabindex and the look, as the place the keys walk from. */
export function prepareStop(
  host: DiagramHost,
  target: TargetKey,
): HTMLElement | SVGElement {
  const place = stopsOf(readingOf(host)).findIndex((stop) =>
    sameTarget(stop, target),
  );
  lastPlace.set(host.container, Math.max(place, 0));
  const el = rove(host, target);
  paint(host, el);
  return el;
}

/** The stops connect mode walks: only its boxes, or its participants. */
function boxStops(stops: TargetKey[]): TargetKey[] {
  return stops.filter(connectable);
}

/** Whether the key now going down is a repeat: a held key crosses nothing (R81). */
const held = new WeakMap<EditorView, boolean>();

/**
 * A peer's edit took a stop off its arrow and put it on the arrow's start box. The
 * person may press without having read that, so the next writing key does nothing but
 * say it again, and no box goes that nobody chose. An arrow key, Escape or the pointer
 * is a choice, and ends this (R102).
 */
const movedStops = new WeakSet<EditorView>();

/** Connect mode ignored a menu key: that key's own `contextmenu` is no right click. */
const keyMenuOwed = new WeakSet<EditorView>();

/** Views whose held key just took a fence: its repeats take nothing more (N6). */
const spentHold = new WeakSet<EditorView>();

/**
 * The chunk's side of the hand-off from the hidden-mark keys (R13). A held key
 * is spent here: it stops at the fence, and only a new press goes in.
 */
export function enterCanvas(main: EditorView, pos: number): void {
  if (held.get(main) !== true) {
    focusTarget(main, pos, CANVAS);
  }
}

/** A stop's fence is back: its element takes the roving tabindex, and a free focus. */
export function backOnStop(
  main: EditorView,
  host: DiagramHost,
  target: TargetKey,
): void {
  const asked = wanted.delete(main);
  const active = main.root.activeElement;
  const el = rove(host, target);
  // The focus went when the old box left, so only an unclaimed one comes back.
  if (asked || active === null || active === main.dom.ownerDocument.body) {
    paint(host, el);
    el.focus({ preventScroll: true });
  }
}

registerIntentReopen("focus", (main, intent, host) => {
  backOnStop(main, host, intent.target);
});

/** Whether the drawing on screen is of another body than the fence's text now. */
function drawingBehind(
  main: EditorView,
  host: DiagramHost,
  from: number,
): boolean {
  return fenceAt(main.state, from)?.body !== host.renderedBody();
}

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
    sayAt(main, from, "stale");
    return true;
  }
  return false;
}

/** The whole fence goes, and a key still held takes nothing more with it. */
export function deleteFenceOf(main: EditorView, from: number): void {
  const outcome = removeFence(main, from);
  if ("refused" in outcome) {
    sayAt(main, from, outcome.refused);
  }
  if (outcome.applied) {
    spentHold.add(main);
  }
}

async function deleteStop(
  main: EditorView,
  from: number,
  target: TargetKey,
): Promise<void> {
  if (target.kind === "canvas") {
    deleteFenceOf(main, from);
    return;
  }
  const fence = fenceAt(main.state, from);
  const read = fence === null ? null : modelOf(fence.body);
  // A peer's duplicate landing during the verify must not move the delete (R94).
  const hold =
    fence !== null && read !== null && isLine(target)
      ? holdLine(main, read, target, fence.bodyFrom)
      : null;
  try {
    const outcome =
      read?.kind === "sequence"
        ? await runDiagramOp(
            main,
            from,
            seqKind,
            seqDelete(main, target, hold, read),
          )
        : await runDiagramOp(
            main,
            from,
            flowKind,
            (model, bodyFrom): FlowOp | { refused: RefusalCode } => {
              if (target.kind === "node") {
                return { op: "deleteNode", id: target.id };
              }
              if (hold === null) {
                return { refused: "gone" };
              }
              const edge = resolveHeldTarget(
                main,
                target,
                hold,
                model,
                bodyFrom,
              );
              return typeof edge === "string"
                ? { refused: edge }
                : edge.kind === "edge"
                  ? { op: "deleteEdge", edge: edge.key }
                  : { refused: "gone" };
            },
          );
    if ("refused" in outcome) {
      sayAt(
        main,
        main.state.field(diagramIntent)?.anchor ?? from,
        outcome.refused,
      );
    }
  } finally {
    if (hold !== null) {
      letGoOfLine(main, hold);
    }
  }
}

/** A sequence delete by key: a participant by name, a message by its held place. */
function seqDelete(
  main: EditorView,
  target: TargetKey,
  hold: LineHold | null,
  read: DiagramModel,
) {
  if (target.kind === "message" && read.kind === "sequence") {
    return heldMessageOp(
      main,
      hold,
      target,
      (message) => ({ op: "deleteMessage", target: message }),
      read,
    );
  }
  if (target.kind === "participant") {
    return heldSeqOp({ op: "deleteParticipant", name: target.name });
  }
  // A box or an arrow has no place in a sequence diagram.
  return () => ({ refused: "gone" as const });
}

/**
 * A fence that ends the document has no line to go on below it, and in a Reflection
 * there is no Raw to make one. Enter on its canvas opens that line, as one undo step,
 * and puts the caret on it (R104). Only Enter: a key that only moves writes nothing.
 */
function openLineBelow(main: EditorView, from: number): void {
  const fence = fenceAt(main.state, from);
  if (fence === null || fence.to !== main.state.doc.length) {
    return;
  }
  const at = fence.to;
  dispatchIsolated(main, {
    changes: { from: at, insert: main.state.lineBreak },
    // A position counts a line break as one, whatever the separator.
    selection: EditorSelection.cursor(at + 1),
    userEvent: "input.diagram",
  });
  // A key still held goes no further than the line it opened.
  spentHold.add(main);
  closeDiagramIntent(main, "user", "none");
  main.focus();
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
  connecting: boolean,
): void {
  const all = stopsOf(readingOf(host));
  const stops = connecting ? boxStops(all) : all;
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
  const target = stops[next < 0 ? here : next];
  if (connecting) {
    if (target !== undefined) {
      focusTarget(main, from, target, "connect");
    }
    return;
  }
  focusTarget(main, from, target ?? CANVAS);
}

/** The keys besides the walk that act on a stop, and Escape. */
const AWAY_KEYS = new Set([
  "Enter",
  "F2",
  " ",
  "Delete",
  "Backspace",
  "Escape",
]);

const WALK_KEYS = new Set([
  "ArrowRight",
  "ArrowDown",
  "ArrowLeft",
  "ArrowUp",
  "Home",
  "End",
]);

/**
 * A Mac's own key for a stop key, read as that key: Cmd+Up and Cmd+Down for Home
 * and End, which need fn on a MacBook, and Cmd+Backspace for Backspace, the
 * Finder's delete (R111). Null off a Mac and for any other key. Cmd+Left and
 * Cmd+Right stay the browser's, as Safari's Back and Forward.
 */
function macStopKey(event: KeyboardEvent): string | null {
  if (
    !isMac() ||
    !event.metaKey ||
    event.ctrlKey ||
    event.altKey ||
    event.shiftKey
  ) {
    return null;
  }
  switch (event.key) {
    case "ArrowUp":
      return "Home";
    case "ArrowDown":
      return "End";
    case "Backspace":
      return "Backspace";
    default:
      return null;
  }
}

/**
 * Whether the browser sends a `contextmenu` event after this menu key. Shift+F10
 * and the menu key do; a Mac's Ctrl+Return is ours alone and sends none.
 */
function sendsContextMenu(event: KeyboardEvent): boolean {
  return event.key === "ContextMenu" || event.key === "F10";
}

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

/** Alt+ArrowUp or Alt+ArrowDown, and no other modifier: a message's move. */
function moveKey(event: KeyboardEvent): boolean {
  return (
    event.altKey &&
    !event.ctrlKey &&
    !event.metaKey &&
    !event.shiftKey &&
    (event.key === "ArrowUp" || event.key === "ArrowDown")
  );
}

/**
 * Moves the message the stop stands on one step, and the stop goes with it. Only on
 * a message: on any other stop the key is not the diagram's. A held key moves once
 * (R81), and a stop a peer moved takes the first press as a reminder (R102).
 */
function onMoveKey(
  main: EditorView,
  host: DiagramHost,
  from: number,
  event: KeyboardEvent,
): boolean {
  const intent = main.state.field(diagramIntent);
  const target = currentTarget(main, from);
  if (intent?.mode === "connect" && intent.anchor === from) {
    return true;
  }
  if (target.kind !== "message") {
    return false;
  }
  if (event.repeat || !host.writable()) {
    return true;
  }
  const direction = event.key === "ArrowUp" ? "up" : "down";
  const turn = moveTurns.get(main) ?? 0;
  const queued = (moveQueues.get(main) ?? Promise.resolve()).then(() =>
    queuedMove(main, direction, turn),
  );
  moveQueues.set(main, queued);
  return true;
}

/** The fence body the person's own last move wrote, per view. */
const ownMoves = new WeakMap<EditorView, string>();
/** Each view's presses, one after another: a press waits for the one before. */
const moveQueues = new WeakMap<EditorView, Promise<void>>();
/** Raised when a press is answered with a sentence: the presses after it are dropped. */
const moveTurns = new WeakMap<EditorView, number>();

function dropQueuedMoves(main: EditorView): void {
  moveTurns.set(main, (moveTurns.get(main) ?? 0) + 1);
}

/**
 * Whether the drawing is behind the text for any reason but the person's own last
 * move. A move writes the text at once and the drawing follows a little later, so a
 * quick next press acts on the text; a peer's change since is still "Try again".
 */
function behindForMove(main: EditorView, from: number): boolean {
  const host = hostOf(main, from);
  const drawn = main.state
    .field(diagramField)
    .fences.find((f) => f.from === from);
  const behind = fenceIsBehind(main.state, from);
  if (behind || (drawn && syntaxTree(main.state).length < drawn.to)) {
    forceParsing(main, drawn?.to ?? from, PARSE_MS);
  }
  const body = fenceAt(main.state, from)?.body;
  if (behind || body === undefined) {
    return true;
  }
  return (
    host !== null && body !== host.renderedBody() && body !== ownMoves.get(main)
  );
}

/**
 * One press's move, run after the presses before it, on the stop as it stands then:
 * the last move put the stop on the message where it landed. A stop a peer moved
 * (R102) or a peer's change since says so, and drops the presses still waiting.
 */
async function queuedMove(
  main: EditorView,
  direction: "up" | "down",
  turn: number,
): Promise<void> {
  if ((moveTurns.get(main) ?? 0) !== turn || !main.dom.isConnected) {
    return;
  }
  const intent = main.state.field(diagramIntent, false);
  if (intent === null || intent === undefined) {
    return;
  }
  const { anchor, target } = intent;
  if (movedStops.delete(main)) {
    dropQueuedMoves(main);
    sayAt(main, anchor, "stale");
    return;
  }
  if (target.kind !== "message" || intent.mode !== "focus") {
    return;
  }
  if (behindForMove(main, anchor)) {
    dropQueuedMoves(main);
    sayAt(main, anchor, "stale");
    return;
  }
  if (!(await moveMessageAt(main, anchor, target, direction))) {
    dropQueuedMoves(main);
  }
}

/**
 * Moves a message one step, through its hold, and puts the stop on it where it
 * lands, so the next press moves it on. `hold` is let go when done; with none, one
 * is taken here.
 */
export async function moveMessageAt(
  main: EditorView,
  from: number,
  target: Extract<TargetKey, { kind: "message" }>,
  direction: "up" | "down",
  given: LineHold | null = null,
): Promise<boolean> {
  const fence = fenceAt(main.state, from);
  const read = fence === null ? null : modelOf(fence.body);
  const hold =
    given ??
    (fence !== null && read !== null
      ? holdLine(main, read, target, fence.bodyFrom)
      : null);
  try {
    const outcome = await runDiagramOp(
      main,
      from,
      seqKind,
      heldMessageOp(
        main,
        hold,
        target,
        (message) => ({ op: "moveMessage", target: message, direction }),
        read?.kind === "sequence" ? read : undefined,
      ),
    );
    const anchor = main.state.field(diagramIntent)?.anchor ?? from;
    if ("refused" in outcome) {
      sayAt(main, anchor, outcome.refused);
      return false;
    }
    if (outcome.applied && outcome.created !== undefined) {
      // The text is what the next press reads, before the drawing catches up.
      const body = fenceAt(main.state, anchor)?.body;
      if (body !== undefined) {
        ownMoves.set(main, body);
      }
      focusTarget(main, anchor, outcome.created);
    }
    return outcome.applied;
  } finally {
    if (hold !== null) {
      letGoOfLine(main, hold);
    }
  }
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
  if (moveKey(event)) {
    return onMoveKey(main, host, from, event);
  }
  // Before the modifier test: Ctrl+Return is a Mac's menu key, and Cmd+Up,
  // Cmd+Down and Cmd+Backspace are its first, last and delete (R111).
  const menu = isMenuKey(event);
  const macKey = macStopKey(event);
  if (
    !menu &&
    macKey === null &&
    (event.ctrlKey || event.metaKey || event.altKey)
  ) {
    return false;
  }
  const key = macKey ?? event.key;
  if (event.shiftKey && !menu) {
    return false;
  }
  // No menu of ours on a page that takes no writes: the browser's own stands.
  if (menu && !host.writable()) {
    return false;
  }
  const intent = main.state.field(diagramIntent);
  const connecting = intent?.mode === "connect" && intent.anchor === from;
  if (key === "Escape") {
    if (!event.repeat) {
      movedStops.delete(main);
      if (connecting) {
        connectHooks?.cancel(main);
      } else {
        leave(main, host, intent);
      }
    }
    return true;
  }
  // Space as a button takes it, or the browser scrolls the page (R87).
  const rename = key === "Enter" || key === "F2" || key === " ";
  const remove = key === "Delete" || key === "Backspace";
  if (!menu && !rename && !remove && !WALK_KEYS.has(key)) {
    return false;
  }
  // Held, the walk may run on within the drawing; nothing else repeats (R81).
  if (event.repeat && !WALK_KEYS.has(key)) {
    return true;
  }
  // Waiting for its box: a walk or a choice, and no other key takes anything.
  if (connecting && (menu || remove || key === "F2")) {
    // By reading only: on Windows that key's `contextmenu` comes after its keyup.
    // A Mac's Ctrl+Return sends none, so nothing is owed for it.
    if (menu && sendsContextMenu(event)) {
      keyMenuOwed.add(main);
    }
    return true;
  }
  // A peer's edit moved the stop here, and this key was meant for the arrow (R102).
  if (movedStops.delete(main) && !WALK_KEYS.has(key)) {
    sayAt(main, from, "stale");
    return true;
  }
  if (staleNow(main, host, from)) {
    return true;
  }
  const target = currentTarget(main, from);
  if (WALK_KEYS.has(key)) {
    walk(main, host, from, key, connecting);
  } else if (connecting) {
    const id = connectIdOf(target);
    if (id !== null) {
      connectHooks?.choose(main, id);
    } else {
      connectHooks?.remind(main);
    }
  } else if (remove) {
    if (host.writable()) {
      void deleteStop(main, from, target);
    }
  } else if (menu) {
    if (host.writable()) {
      askMenu(
        main,
        () => main.state.field(diagramIntent)?.anchor ?? from,
        target,
      );
    }
  } else if (target.kind === "canvas") {
    if (key === "Enter" && host.writable()) {
      openLineBelow(main, from);
    }
  } else {
    stopActions.get("rename")?.(main, from, target, host);
  }
  return true;
}

/**
 * A rename field stands first: its draft is written, then the menu may open, on the
 * fence `where` finds then, as that draft can move it.
 */
function askMenu(
  main: EditorView,
  where: () => number | null,
  target: TargetKey,
  at?: { x: number; y: number },
): void {
  throughGate(main, where, target, (anchor, now) => {
    const host = hostOf(main, anchor);
    if (host !== null) {
      stopActions.get("menu")?.(main, anchor, now, host, at);
    }
  });
}

/**
 * Runs `act` once a rename field has written its draft, which waits for a verify. A
 * peer's duplicate can land in that wait, so an arrow is held by its place from now,
 * and `act` gets it as it stands then, or the person a sentence (R94).
 */
function throughGate(
  main: EditorView,
  where: () => number | null,
  target: TargetKey,
  act: (anchor: number, target: TargetKey) => void,
): void {
  const at = where();
  const fence = at === null ? null : fenceAt(main.state, at);
  const hold =
    isLine(target) && fence !== null
      ? holdLine(main, modelOf(fence.body), target, fence.bodyFrom)
      : null;
  const letGo = () => {
    if (hold !== null) {
      letGoOfLine(main, hold);
    }
  };
  const go = () => {
    const anchor = where();
    if (anchor === null) {
      letGo();
      return;
    }
    let now = target;
    if (hold !== null) {
      const stands = fenceAt(main.state, anchor);
      const line =
        stands === null
          ? "gone"
          : resolveHeldTarget(
              main,
              target,
              hold,
              modelOf(stands.body),
              stands.bodyFrom,
            );
      letGo();
      if (typeof line === "string") {
        sayAt(main, anchor, line);
        return;
      }
      now = line;
    }
    act(anchor, now);
  };
  if (clickGate === null) {
    go();
  } else {
    clickGate(main, target, go, letGo);
  }
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
    decorateDrawing(reading.svg, reading.svgId, reading.model, reading.verdict);
  }
  // Only a page that takes writes has a menu to offer (R87).
  if (reading !== null) {
    for (const el of reading.svg.querySelectorAll('[role="button"]')) {
      el.toggleAttribute("aria-haspopup", false);
      if (host.writable()) {
        el.setAttribute("aria-haspopup", "menu");
      }
    }
  }
  const intent = main.state.field(diagramIntent);
  if (intent === null || hostOf(main, intent.anchor) !== host) {
    rove(host, CANVAS);
    return;
  }
  if (intent.mode === "rename") {
    return;
  }
  // The new drawing's elements. The menu holds the focus, and only the look moves.
  if (intent.mode === "menu") {
    paint(host, rove(host, intent.target));
    return;
  }
  let { target } = intent;
  const stops =
    intent.mode === "connect" ? boxStops(stopsOf(reading)) : stopsOf(reading);
  if (!stops.some((stop) => sameTarget(stop, target))) {
    // The stop went, by a delete or a peer: the one now in its place, never the
    // body, and only a drawing of the text as it stands says it went. A message
    // renamed a moment ago is not in the old drawing yet.
    if (reading === null || drawingBehind(main, host, intent.anchor)) {
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
  return reading === null ? CANVAS : stopAtOffset(reading.model, offset);
}

/** Whether a sentence in this box's status line stands inside the window. */
function noticeInWindow(host: DiagramHost): boolean {
  const status = host.container.querySelector(".cm-diagram-notice");
  if (status === null) {
    return false;
  }
  const { top, bottom } = status.getBoundingClientRect();
  if (top < 0 || bottom > window.innerHeight) {
    return false;
  }
  for (let el = status.parentElement; el !== null; el = el.parentElement) {
    if (/auto|scroll|hidden/.test(getComputedStyle(el).overflowY)) {
      const clip = el.getBoundingClientRect();
      if (clip.height > 0 && (top < clip.top || bottom > clip.bottom)) {
        return false;
      }
    }
  }
  return true;
}

/**
 * Says a refusal where the person can see it: the box's line when that stands in the
 * window, the panel otherwise, and a screen reader hears it either way (R89, R92). A
 * view that went has nobody to tell.
 */
export function sayAt(
  main: EditorView,
  anchor: number,
  code: RefusalCode,
): void {
  if (!main.dom.isConnected) {
    return;
  }
  const host = anchor <= main.state.doc.length ? hostOf(main, anchor) : null;
  if (host === null || !noticeInWindow(host)) {
    main.dispatch({ effects: sayRefusal.of(code) });
    return;
  }
  // A live line that does not change is not read again.
  const same =
    host.container.querySelector(".cm-diagram-notice")?.textContent ===
    NOTICES[code];
  host.notice(code);
  if (same) {
    main.dispatch({ effects: EditorView.announce.of(NOTICES[code]) });
  }
}

/** The views this layer runs in: a filter has only a state to go by. */
const liveViews = new Set<EditorView>();

/** Whether the view showing `state` has the focus on its own text. */
function mainHoldsFocus(state: EditorState): boolean {
  for (const view of liveViews) {
    if (view.state === state) {
      return view.root.activeElement === view.contentDOM;
    }
  }
  return false;
}

const refusalShown = StateEffect.define<string | null>();

/** A refusal with no box left to say it in: it goes to the panel below. */
export const sayRefusal = StateEffect.define<RefusalCode>();

/** The sentence of a refusal whose fence is out of the window, in a panel. */
const refusalPanel = StateField.define<string | null>({
  create: () => null,
  update: (value, tr) => {
    let next = value;
    for (const effect of tr.effects) {
      if (effect.is(refusalShown)) {
        next = effect.value;
      }
    }
    return next;
  },
  provide: (field) =>
    showPanel.from(field, (sentence) =>
      sentence === null
        ? null
        : () => {
            const dom = document.createElement("div");
            dom.className = "cm-diagram-refusal";
            dom.setAttribute("role", "status");
            dom.textContent = sentence;
            return { dom };
          },
    ),
});

/** Keeps the keyboard's places and the look current, and takes the DOM events. */
const focusPlugin = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;
    readonly stop: (() => void)[];
    /** The box wearing a search hit's look. */
    lookOn: DiagramHost | null = null;
    /** Takes a refusal's panel away again. */
    panelTimer: ReturnType<typeof setTimeout> | null = null;
    /** The arrow or message the keyboard stands on, by its place. */
    stopHold: LineHold | null = null;
    /** The person's own edit came: it may rewrite the token, and it means no harm. */
    ownEdit = false;
    /** This plugin ended, as Raw ends it. A view that comes back gets a new one. */
    ended = false;
    /** Whether `awayKey` listens: a stop or a mode is open and its box is not there. */
    awayArmed = false;
    /**
     * A stop has had the focus since the person last pressed anywhere. Only then is
     * a key on the body theirs for the diagram: after a press elsewhere it is the
     * page's own, a scroll key above all (R105, N1).
     */
    stopFocused = false;
    readonly pointerAway = (): void => {
      this.stopFocused = false;
    };

    /**
     * A key with the focus on the body while a stop's box is away (scrolled out of
     * the window) or connect mode waits: a diagram key brings the box back and puts
     * the focus on the stop, and does nothing else, as the drawing was not in view
     * (R105, as R102). Escape keeps its meaning: it ends what is open.
     */
    readonly awayKey = (event: KeyboardEvent): void => {
      const { view } = this;
      const active = view.root.activeElement;
      const move = moveKey(event);
      // Read before the modifier test, as `onKey` reads them (R111).
      const menu = isMenuKey(event);
      const macKey = macStopKey(event);
      if (
        (active !== null && active !== view.dom.ownerDocument.body) ||
        ((event.ctrlKey || event.metaKey) && !menu && macKey === null) ||
        (event.altKey && !move) ||
        event.isComposing
      ) {
        return;
      }
      const key = macKey ?? event.key;
      if (
        (event.shiftKey && !menu) ||
        !(move || menu || WALK_KEYS.has(key) || AWAY_KEYS.has(key))
      ) {
        return;
      }
      const intent = view.state.field(diagramIntent, false);
      if (intent === null || intent === undefined || intent.mode === "rename") {
        return;
      }
      const host = hostOf(view, intent.anchor);
      // Connect mode with its box there, or after the person went elsewhere, takes
      // only its way out.
      if (key !== "Escape" && (host !== null || !this.stopFocused)) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) {
        return;
      }
      if (key === "Escape") {
        if (intent.mode === "connect") {
          connectHooks?.cancel(view);
        } else {
          closeDiagramIntent(view, "user");
        }
        return;
      }
      if (host === null) {
        wanted.add(view);
        view.dispatch({
          effects: EditorView.scrollIntoView(intent.anchor, { y: "center" }),
        });
      } else {
        prepareStop(host, intent.target).focus({ preventScroll: true });
      }
    };

    /** Listens only while it can matter, so a page with no diagram pays nothing. */
    syncAway(): void {
      const { view } = this;
      const intent = this.ended ? null : view.state.field(diagramIntent, false);
      const want =
        intent !== null &&
        intent !== undefined &&
        intent.mode !== "rename" &&
        (intent.mode === "connect" ||
          (this.stopFocused && hostOf(view, intent.anchor) === null));
      if (want !== this.awayArmed) {
        this.awayArmed = want;
        if (want) {
          window.addEventListener("keydown", this.awayKey, true);
        } else {
          window.removeEventListener("keydown", this.awayKey, true);
        }
      }
    }

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
      keyMenuOwed.delete(this.view);
      movedStops.delete(this.view);
      const el = event.target;
      if (
        el instanceof Element &&
        el.closest(".cm-diagram") === null &&
        this.view.state.field(diagramIntent) !== null &&
        this.view.state.field(diagramIntent)?.released !== true
      ) {
        closeDiagramIntent(this.view, "user", "none");
      }
    };
    readonly click = (event: Event): void => {
      movedStops.delete(this.view);
      const place = this.place(event);
      if (place === null) {
        return;
      }
      if (this.connectClick(event, place)) {
        return;
      }
      if (staleNow(this.view, place.host, place.from)) {
        return;
      }
      const reading = readingOf(place.host);
      const el = event.target as Element;
      const target =
        reading === null
          ? CANVAS
          : targetOfElement(el, reading.svgId, reading.model, reading.verdict);
      const { view } = this;
      // Found again from the box, as a rename field's draft may move it. The intent's
      // own anchor would be the other fence's when the click came from there.
      const box = place.host.container;
      throughGate(
        view,
        () => (box.isConnected ? fenceFromOf(view, box) : null),
        target,
        (from, now) => {
          focusTarget(view, from, now);
          if (now.kind !== "canvas" && writable(view.state)) {
            const host = hostOf(view, from);
            if (host !== null) {
              stopActions.get("rename")?.(view, from, now, host);
            }
          }
        },
      );
    };
    readonly contextmenu = (event: Event): void => {
      movedStops.delete(this.view);
      const place = this.place(event);
      // A page that takes no writes has no menu of ours, and keeps the browser's.
      if (place === null || !place.host.writable()) {
        return;
      }
      if (!stopActions.has("menu")) {
        return;
      }
      event.preventDefault();
      const intent = this.view.state.field(diagramIntent);
      if (keyMenuOwed.delete(this.view)) {
        return;
      }
      if (intent?.mode === "connect" && intent.anchor === place.from) {
        connectHooks?.cancel(this.view);
        return;
      }
      if (staleNow(this.view, place.host, place.from)) {
        return;
      }
      const reading = readingOf(place.host);
      const mouse = event as MouseEvent;
      const box = place.host.container;
      askMenu(
        this.view,
        () => (box.isConnected ? fenceFromOf(this.view, box) : null),
        reading === null
          ? CANVAS
          : targetOfElement(
              event.target as Element,
              reading.svgId,
              reading.model,
              reading.verdict,
            ),
        // A key's own event has no place of its own.
        mouse.clientX === 0 && mouse.clientY === 0
          ? undefined
          : { x: mouse.clientX, y: mouse.clientY },
      );
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
    readonly noteKey = (event: KeyboardEvent): void => {
      held.set(this.view, event.repeat);
      if (!event.repeat) {
        spentHold.delete(this.view);
      } else if (spentHold.has(this.view)) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
    };
    readonly keyUp = (): void => {
      spentHold.delete(this.view);
      if (held.get(this.view) === true) {
        held.set(this.view, false);
        this.ruleOnSelection();
      }
    };
    readonly focusIn = (event: Event): void => {
      if (event.target === this.view.contentDOM) {
        queueMicrotask(() => {
          this.mainFocused();
        });
        return;
      }
      const box = boxOf(event);
      if (box === null) {
        return;
      }
      this.stopFocused = true;
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
        this.stopFocused = false;
        queueMicrotask(() => {
          this.left(box);
        });
      }
    };

    constructor(view: EditorView) {
      this.view = view;
      liveViews.add(view);
      const dom = view.contentDOM;
      dom.addEventListener("mousedown", this.mousedown);
      view.dom.addEventListener("mousedown", this.pressOutside, true);
      view.dom.addEventListener("keydown", this.noteKey, true);
      view.dom.addEventListener("keyup", this.keyUp, true);
      dom.addEventListener("click", this.click);
      dom.addEventListener("contextmenu", this.contextmenu);
      dom.addEventListener("keydown", this.keydown);
      dom.addEventListener("focus", this.focusIn, true);
      dom.addEventListener("blur", this.focusOut, true);
      // Anywhere: a press outside the editor is a person going elsewhere too.
      window.addEventListener("pointerdown", this.pointerAway, true);
      this.stop = [
        onDiagramDrawn(view, (host) => {
          settle(view, host);
        }),
        onBoxMounted(view, (host) => {
          settle(view, host);
          this.syncAway();
        }),
      ];
    }

    /**
     * A click while connect mode waits: a box is its answer, the canvas cancels and
     * an arrow is no answer. Cancelling asks nothing of the drawing, as Escape does.
     */
    connectClick(
      event: Event,
      place: { host: DiagramHost; from: number },
    ): boolean {
      const { view } = this;
      const intent = view.state.field(diagramIntent);
      if (intent?.mode !== "connect" || intent.anchor !== place.from) {
        return false;
      }
      const reading = readingOf(place.host);
      const target =
        reading === null
          ? CANVAS
          : targetOfElement(
              event.target as Element,
              reading.svgId,
              reading.model,
              reading.verdict,
            );
      const id = connectIdOf(target);
      if (target.kind === "canvas") {
        connectHooks?.cancel(view);
      } else if (id === null) {
        connectHooks?.remind(view);
      } else if (!staleNow(view, place.host, place.from)) {
        connectHooks?.choose(view, id);
      }
      return true;
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
      // Gone on already: to the rename field a click opens in the same tick.
      if (
        host === null ||
        from === null ||
        !el.isConnected ||
        view.root.activeElement !== el
      ) {
        return;
      }
      const reading = readingOf(host);
      const target =
        reading === null || el === box
          ? CANVAS
          : targetOfElement(el, reading.svgId, reading.model, reading.verdict);
      const intent = view.state.field(diagramIntent);
      // The box stands in for a stop the drawing has not caught up with, as a
      // message just renamed: the stop stays, and the next drawing takes it.
      if (
        el === box &&
        intent !== null &&
        intent.anchor === from &&
        intent.target.kind !== "canvas" &&
        drawingBehind(view, host, from)
      ) {
        paint(host, box);
        return;
      }
      // Connect mode keeps its mode, and stands only on a box.
      if (intent?.mode === "connect" && intent.anchor === from) {
        if (connectable(target) && !sameTarget(intent.target, target)) {
          view.dispatch({
            effects: setDiagramIntent.of({ ...intent, target }),
          });
        }
        paint(host, rove(host, connectable(target) ? target : intent.target));
        return;
      }
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

    /** Stop holding the stop's arrow. */
    dropStopHold(): void {
      if (this.stopHold !== null) {
        letGoOfLine(this.view, this.stopHold);
        this.stopHold = null;
      }
    }

    /**
     * A stop on an arrow or a message is a choice too: it follows that line when a
     * duplicate moves its occurrence, and leaves when it cannot tell, so the ring
     * never comes to stand on another line that the next key would take.
     */
    followStop(): void {
      const { view } = this;
      const intent = view.state.field(diagramIntent, false);
      const target =
        intent?.mode === "focus" && isLine(intent.target)
          ? intent.target
          : null;
      if (intent === null || intent === undefined || target === null) {
        this.ownEdit = false;
        this.dropStopHold();
        return;
      }
      const fence = fenceAt(view.state, intent.anchor);
      if (fence === null || fenceIsBehind(view.state, intent.anchor)) {
        return;
      }
      const model = modelOf(fence.body);
      if (!model.understood) {
        return;
      }
      const own = this.ownEdit;
      this.ownEdit = false;
      const hold = this.stopHold;
      const now =
        hold === null || !sameTarget(heldTargetOf(hold), target)
          ? null
          : resolveHeldTarget(view, target, hold, model, fence.bodyFrom);
      // A new stop, or the person's own edit took the token, as a Line and its undo
      // do. That is the same line, so the ticket goes back in it.
      if (hold === null || now === null || (own && typeof now === "string")) {
        this.dropStopHold();
        if (labelOfTarget(model, target) !== null) {
          this.stopHold = holdLine(view, model, target, fence.bodyFrom);
        }
        return;
      }
      if (now === "gone") {
        // `settle` gives the stop to what stands in its place.
        this.dropStopHold();
      } else if (now === "stale") {
        this.dropStopHold();
        view.dispatch({
          effects: setDiagramIntent.of({
            ...intent,
            target: startOf(target) ?? CANVAS,
          }),
        });
        movedStops.add(view);
        sayAt(view, intent.anchor, "stale");
      } else if (!sameTarget(now, target)) {
        followLine(hold, now);
        view.dispatch({
          effects: setDiagramIntent.of({ ...intent, target: now }),
        });
      }
    }

    update(update: ViewUpdate): void {
      const { view } = this;
      if (
        update.docChanged ||
        update.startState.field(diagramIntent) !==
          update.state.field(diagramIntent)
      ) {
        // A room's undo and redo reach the text with no user event, as a peer's edit
        // does. Only the manager says they are the person's own.
        const room = update.state.facet(roomUndo);
        this.ownEdit ||= update.transactions.some(
          (tr) =>
            tr.docChanged &&
            (tr.annotation(Transaction.userEvent) !== undefined ||
              room?.undoing === true ||
              room?.redoing === true),
        );
        queueMicrotask(() => {
          if (!this.ended) {
            this.followStop();
          }
        });
      }
      if (
        this.awayArmed ||
        update.viewportChanged ||
        update.state.field(diagramIntent) !== null
      ) {
        queueMicrotask(() => {
          this.syncAway();
        });
      }
      if (
        update.transactions.some((tr) =>
          tr.effects.some((e) => e.is(verdictArrived)),
        )
      ) {
        queueMicrotask(() => {
          this.redecorate();
        });
      }
      if (writable(update.startState) !== writable(update.state)) {
        queueMicrotask(() => {
          this.redecorate();
        });
      }
      for (const tr of update.transactions) {
        for (const effect of tr.effects) {
          if (effect.is(sayRefusal)) {
            const code = effect.value;
            queueMicrotask(() => {
              this.sayInPanel(code);
            });
          }
          if (effect.is(fenceEditRefused)) {
            const refused = effect.value;
            queueMicrotask(() => {
              this.refused(refused);
            });
          }
        }
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
        } else {
          this.ruleOnSelection();
        }
      });
    }

    /** A selection in a drawn fence with main focused: the canvas takes over. */
    ruleOnSelection(): void {
      const { view } = this;
      const { from, to } = view.state.selection.main;
      if (
        held.get(view) !== true &&
        view.hasFocus &&
        !view.composing &&
        view.state
          .facet(drawnFenceLines)
          .some((f) => f.from <= from && to <= f.to)
      ) {
        focusTarget(view, from, CANVAS);
      }
    }

    /**
     * Main took the focus with its selection in a drawn fence: by Escape from a
     * search, or Shift+Tab off the box. The fence shows as text, so what the next
     * key writes can be seen (R80).
     */
    mainFocused(): void {
      const { view } = this;
      if (!view.hasFocus) {
        return;
      }
      const { state } = view;
      // A field writing its draft has the last word on this box.
      if (state.field(diagramIntent)?.mode === "rename") {
        return;
      }
      const { from, to } = state.selection.main;
      const lines = state
        .facet(drawnFenceLines)
        .find((f) => f.from <= from && to <= f.to);
      const fence =
        lines &&
        state
          .field(diagramField)
          .fences.find((f) => lines.from <= f.from && f.to <= lines.to);
      if (fence) {
        view.dispatch({ effects: showFenceSource.of(fence.from) });
      }
    }

    /** What a person hears about an edit `fenceMarksGuard` stopped. */
    refused({ at, handOff, say, code }: FenceEditRefusal): void {
      const { view } = this;
      if (handOff) {
        focusTarget(view, at, CANVAS);
      }
      if (say) {
        const host = hostOf(view, at);
        if (host !== null && noticeInWindow(host)) {
          host.notice(code);
        } else {
          this.sayInPanel(code);
        }
      }
    }

    /**
     * The fence is not where the person is looking, and a refusal must still
     * reach them: a panel of the editor stays in the window, as the search panel
     * does, and the page does not move for it (R89).
     */
    sayInPanel(code: RefusalCode): void {
      const { view } = this;
      // Announced as well: a live region that arrives with its text is commonly
      // not read, and a second refusal changes nothing in the panel (R92).
      view.dispatch({
        effects: [
          refusalShown.of(NOTICES[code]),
          EditorView.announce.of(NOTICES[code]),
        ],
      });
      if (this.panelTimer !== null) {
        clearTimeout(this.panelTimer);
      }
      this.panelTimer = setTimeout(() => {
        this.panelTimer = null;
        view.dispatch({ effects: refusalShown.of(null) });
      }, NOTICE_MS);
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
      this.ended = true;
      this.syncAway();
      // Not during the reconfigure that ends this: Raw keeps the anchors, and the view.
      queueMicrotask(() => {
        this.dropStopHold();
      });
      liveViews.delete(this.view);
      const dom = this.view.contentDOM;
      dom.removeEventListener("mousedown", this.mousedown);
      this.view.dom.removeEventListener("mousedown", this.pressOutside, true);
      this.view.dom.removeEventListener("keydown", this.noteKey, true);
      this.view.dom.removeEventListener("keyup", this.keyUp, true);
      dom.removeEventListener("click", this.click);
      dom.removeEventListener("contextmenu", this.contextmenu);
      dom.removeEventListener("keydown", this.keydown);
      dom.removeEventListener("focus", this.focusIn, true);
      dom.removeEventListener("blur", this.focusOut, true);
      window.removeEventListener("pointerdown", this.pointerAway, true);
      for (const stop of this.stop) {
        stop();
      }
      if (this.panelTimer !== null) {
        clearTimeout(this.panelTimer);
      }
    }
  },
);

export const diagramFocus: Extension = [focusPlugin, refusalPanel];

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

/** The guard's answer: its transaction carries this instead of the edit. */
interface FenceEditRefusal {
  /** A position in the fence. */
  at: number;
  /** The edit came from a selection inside the fence: the box takes the keyboard. */
  handOff: boolean;
  say: boolean;
  /** What is said. */
  code: RefusalCode;
}

const fenceEditRefused = StateEffect.define<FenceEditRefusal>();

/**
 * Refuses a person's edit that would break a drawn fence's marks: a line joined
 * to one, a line moved past one (R73), a prefix or a Replace all that reaches
 * one. Also an edit a selection inside a drawn fence makes, except the search
 * panel's Replace, which names its text: the rest would skip the op layer and
 * its verify (R80). An edit inside the body or across the whole fence passes,
 * and so do a peer's edit and an undo, which carry no such event. A refusal
 * beats a wrong edit, and it says so, except for a lone key at a line's edge,
 * which is spent like the code box's (R82).
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
    const { doc, selection } = tr.startState;
    const loneKey =
      tr.isUserEvent("move.line") ||
      ((tr.isUserEvent("delete.backward") ||
        tr.isUserEvent("delete.forward")) &&
        selection.ranges.every((range) => range.empty));
    for (const fence of tr.startState.facet(drawnFenceLines)) {
      const atFence = tr.changes.touchesRange(fence.from, fence.to) !== false;
      // Only what main's selection is really making. At the fence that is plain.
      // Elsewhere it is a typed letter with main holding the focus, as the
      // browser's caret is not this selection (R90). Any other edit elsewhere (a
      // table cell, a popover, a drop) has nothing to do with it (R91).
      if (
        !tr.isUserEvent("input.replace") &&
        (atFence ||
          (tr.isUserEvent("input.type") && mainHoldsFocus(tr.startState))) &&
        selection.ranges.some(
          (range) =>
            fence.from <= range.from &&
            range.to <= fence.to &&
            (range.from !== fence.from || range.to !== fence.to),
        )
      ) {
        return {
          effects: fenceEditRefused.of({
            at: fence.to,
            handOff: true,
            say: false,
            code: "breaksFence",
          }),
        };
      }
      if (!atFence) {
        continue;
      }
      let whole = false;
      tr.changes.iterChanges((fromA, toA) => {
        whole ||= fromA <= fence.from && fence.to <= toA;
      });
      if (whole) {
        continue;
      }
      // A fence mark written into the body ends the fence early.
      let closes = false;
      tr.changes.iterChanges((_fromA, _toA, _fromB, _toB, inserted) => {
        closes ||= /(?:^|\n)[ \t]*(?:`{3,}|~{3,})/.test(inserted.toString());
      });
      const from = tr.changes.mapPos(fence.from, 1);
      const to = tr.changes.mapPos(fence.to, -1);
      const first = tr.newDoc.lineAt(from);
      const last = tr.newDoc.lineAt(to);
      // A fence with a line below must keep one: last, it has no way below (R104).
      const endsDocument = fence.to < doc.length && to >= tr.newDoc.length;
      if (
        endsDocument ||
        closes ||
        first.from !== from ||
        last.to !== to ||
        first.text !== doc.lineAt(fence.from).text ||
        last.text !== doc.lineAt(fence.to).text
      ) {
        return {
          effects: fenceEditRefused.of({
            at: fence.to,
            handOff: false,
            say: !loneKey,
            code:
              endsDocument && !closes && first.from === from && last.to === to
                ? "needsLineBelow"
                : "breaksFence",
          }),
        };
      }
    }
    return tr;
  },
);
