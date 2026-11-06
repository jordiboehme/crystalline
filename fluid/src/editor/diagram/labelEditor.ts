/**
 * The rename field: a textarea in the box's overlay, over the box or arrow it
 * names. It is part of the page, not the document, so nothing in it reaches the
 * text until a commit, which goes through `runDiagramOp` as one undo step.
 *
 * Where it lives. The overlay is the box's own layer, and a new drawing lands in
 * the layer beside it, so a redraw moves the field (a style change) and touches
 * nothing else: no focus, no selection, no composition. A box that scrolled away
 * took the element out of the page; this record keeps it, with its text and
 * caret, and the intent's reopener puts it back.
 *
 * What the text decides. The drawing on screen can be behind the text
 * (`renderedBody` does not show an edit while the parse is behind), so the label,
 * and whether the target still exists, are read from the fence as it stands. An
 * answer that needs a parse waits for it (`intentStale`), and one that would move
 * a person's composition waits for its end.
 *
 * How it ends:
 * - Enter commits and the focus goes back to the stop.
 * - Escape writes nothing and the focus goes back to the stop.
 * - A blur commits, and the focus stays where it went. A window losing the focus
 *   is no blur: the browser hands the focus back along with the field.
 * - A close from elsewhere (a press in the text, Raw on) commits what is typed.
 * - `setState` drops it: another document.
 * - A refusal keeps it open with its draft, marked, and says why. It never takes
 *   the focus back after a blur, so a person can always go on (R96); only leaving
 *   the page is held, as that would lose the draft.
 *
 * Leaving (R95). While a field is open a link, Done and Back wait for its commit,
 * and a refused one stays on the page. Closing the tab cannot wait, so
 * `beforeunload` asks. The view going away with nothing having held it is the one
 * route that still drops a draft.
 *
 * An arrow is held by its place in the text, not by its occurrence, which a peer's
 * duplicate moves (R94): `flowEdgeAt`, read again on each pass of the op.
 *
 * With a peer: a body edit keeps the field, and the fence stays drawn while it is
 * open whatever mermaid says of a half-typed line (`keepFenceDrawn`). Two people
 * renaming a box is last writer wins, except that a field nobody typed in shows the
 * peer's new name, so an Enter there cannot put the old one back.
 */

import { forceParsing, syntaxTree } from "@codemirror/language";
import { StateEffect, StateField, Transaction } from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin, showPanel } from "@codemirror/view";

import { backGates } from "../../backGates";
import { diagramAnchors, trackAnchor, untrackAnchor } from "../diagramAnchors";
import { writable } from "../elementEdit";
import { flowKind, nextAnchorTicket, runDiagramOp } from "./apply";
import type { OpResolver } from "./apply";
import type { RefusalCode } from "./edits";
import { NOTICES } from "./edits";
import type { MermaidFence } from "./fence";
import { fenceAt } from "./fence";
import {
  diagramField,
  drawUnderCaret,
  keepFenceDrawn,
  verdictArrived,
  showFenceSource,
} from "./field";
import { flowEdgeAt, flowEdgeSpanOf, flowLabelOf } from "./flowchart/hit";
import { tidyLabel } from "./flowchart/labels";
import type { FlowModel } from "./flowchart/model";
import type { FlowOp } from "./flowchart/ops";
import { compileFlowOp } from "./flowchart/ops";
import {
  clearStopLook,
  focusTarget,
  modelOf,
  registerClickGate,
  registerStopAction,
  sayRefusal,
  targetElement,
} from "./focus";
import type { DiagramIntent, IntentCloseReason, TargetKey } from "./intent";
import {
  beforeClose,
  closeDiagramIntent,
  diagramIntent,
  intentStale,
  registerIntentReopen,
  setDiagramIntent,
} from "./intent";
import { verdictFor } from "./verdict";
import type { DiagramHost } from "./widget";
import { hostOf, onDiagramDrawn } from "./widget";

const CANVAS: TargetKey = { kind: "canvas" };
const FIELD_CLASS = "cm-diagram-label-field";
/** Room to type in, over a small box or an arrow's midpoint. */
const MIN_WIDTH = 128;
const MIN_HEIGHT = 28;
/** How tall a long label may make the field before it scrolls inside. */
const MAX_HEIGHT = 160;
const PARSE_MS = 50;

/** How a commit or a close was asked for, which says where the focus goes. */
type Way = "enter" | "blur" | "leave" | "switch";

interface OpenField {
  main: EditorView;
  el: HTMLTextAreaElement;
  target: TargetKey;
  /** Where the fence is: the intent's anchor, followed while the intent is away. */
  anchor: number;
  /** The label the person saw: an unchanged text writes nothing. */
  baseline: string;
  /** Whether the person changed it, so a peer's rename may not replace it. */
  typed: boolean;
  composing: boolean;
  /** A close waits for the composition to end. */
  goneWaits: boolean;
  /** The commit running now. */
  checking: Promise<void> | null;
  /** The ticket holding an arrow's place in the text (R94); null for a box. */
  arrow: number | null;
  /** What a refusal said, while the text it refused still stands. */
  refused: RefusalCode | null;
  /** Letters typed while the box was away: they go in when it is back (R98). */
  queued: string;
  /** Where the field stood when a composition began, to hold the page there. */
  lockTop: number | null;
}

const fields = new WeakMap<EditorView, OpenField>();

const sameTarget = (a: TargetKey, b: TargetKey) =>
  JSON.stringify(a) === JSON.stringify(b);

/** Says a refusal where it can be seen: the box, or the panel with no box. */
function say(main: EditorView, code: RefusalCode, host: DiagramHost | null) {
  if (host === null) {
    main.dispatch({ effects: sayRefusal.of(code) });
  } else {
    host.notice(code);
  }
}

/**
 * The arrow's key in `model`, by the place the field holds. A box's target stands
 * as it is, and `flowLabelOf` says whether it is there.
 */
function heldTarget(
  main: EditorView,
  target: TargetKey,
  arrow: number | null,
  model: FlowModel,
  bodyFrom: number,
): TargetKey | "stale" | "gone" {
  if (target.kind !== "edge") {
    return target;
  }
  const pos =
    arrow === null
      ? undefined
      : main.state.field(diagramAnchors, false)?.get(arrow);
  const edge = flowEdgeAt(
    model,
    target.key,
    pos === undefined ? null : pos - bodyFrom,
  );
  return typeof edge === "string" ? edge : { kind: "edge", key: edge };
}

/** The fence at `anchor` on a tree that reaches it, or null. */
function fenceNow(main: EditorView, anchor: number): MermaidFence | null {
  if (syntaxTree(main.state).length < main.state.doc.length) {
    forceParsing(main, main.state.doc.length, PARSE_MS);
  }
  const reach = Math.min(anchor, main.state.doc.length);
  return fenceAt(main.state, reach);
}

/** The model of the fence as it stands, when it is one the verdict can read. */
function modelNow(fence: MermaidFence): FlowModel | null {
  const model = modelOf(fence.body);
  return model.understood ? model : null;
}

/** The box or arrow's place in the overlay's coordinates. */
function placeOf(
  target: TargetKey,
  host: DiagramHost,
): { left: number; top: number; width: number; height: number } | null {
  const el = targetElement(host, target);
  if (el === null) {
    return null;
  }
  const frame = host.overlay.getBoundingClientRect();
  const from = (rect: {
    left: number;
    top: number;
    width: number;
    height: number;
  }) => ({
    left: rect.left - frame.left,
    top: rect.top - frame.top,
    width: rect.width,
    height: rect.height,
  });
  if (target.kind !== "edge") {
    return from(el.getBoundingClientRect());
  }
  // An arrow's label where it has one, its path's midpoint otherwise.
  const id = el.getAttribute("data-id");
  const label = [...(host.svg()?.querySelectorAll("g.label[data-id]") ?? [])]
    .find((part) => part.getAttribute("data-id") === id)
    ?.getBoundingClientRect();
  if (label !== undefined && label.width > 0) {
    return from(label);
  }
  const path = el as SVGGeometryElement;
  if (typeof path.getPointAtLength !== "function") {
    return from(el.getBoundingClientRect());
  }
  const point = path.getPointAtLength(path.getTotalLength() / 2);
  const screen = path.getScreenCTM();
  const at = screen === null ? point : point.matrixTransform(screen);
  return from({ left: at.x, top: at.y, width: 0, height: 0 });
}

/** Stands the field over its target, or in the box's middle before it is drawn. */
function placeField(open: OpenField, host: DiagramHost): void {
  const { el } = open;
  const frame = host.overlay.getBoundingClientRect();
  const place = placeOf(open.target, host) ?? {
    left: frame.width / 2,
    top: frame.height / 2,
    width: 0,
    height: 0,
  };
  const width = Math.min(
    Math.max(place.width, MIN_WIDTH),
    Math.max(frame.width, MIN_WIDTH),
  );
  el.style.width = `${String(width)}px`;
  el.style.height = "auto";
  // Its text's own height, in the middle of a tall box: where the label was.
  const height = Math.min(Math.max(el.scrollHeight, MIN_HEIGHT), MAX_HEIGHT);
  el.style.height = `${String(height)}px`;
  el.style.overflow = el.scrollHeight > height ? "auto" : "hidden";
  const left = place.left + place.width / 2 - width / 2;
  el.style.left = `${String(Math.max(0, Math.min(left, frame.width - width)))}px`;
  el.style.top = `${String(place.top + place.height / 2 - height / 2)}px`;
}

/** Whether the field may take the focus back: nothing else asked for it. */
function focusIsFree(open: OpenField, host: DiagramHost | null): boolean {
  const { main } = open;
  const active = main.root.activeElement;
  return (
    active === null ||
    active === main.dom.ownerDocument.body ||
    (host !== null && host.container.contains(active))
  );
}

function mount(
  open: OpenField,
  host: DiagramHost,
  how: "fresh" | "select" | "back",
): void {
  const { el } = open;
  if (el.parentElement !== host.overlay) {
    host.overlay.append(el);
  }
  clearStopLook(host);
  placeField(open, host);
  if (open.refused !== null) {
    showRefusal(open, host);
  }
  if (how === "back") {
    // A box coming back: the focus went with the old one, or went somewhere.
    if (
      focusIsFree(open, host) &&
      (open.refused === null || open.queued !== "")
    ) {
      el.focus({ preventScroll: true });
    }
    if (open.queued !== "") {
      const { queued } = open;
      open.queued = "";
      el.setRangeText(queued, el.selectionStart, el.selectionEnd, "end");
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }
    return;
  }
  el.focus();
  if (how === "select") {
    el.select();
  } else {
    el.setSelectionRange(el.value.length, el.value.length);
  }
}

/** The field out of the record and the page. The caller says where the focus goes. */
function detach(open: OpenField): void {
  const { main, arrow } = open;
  fields.delete(main);
  open.el.remove();
  syncGuards(main);
  const effects: StateEffect<unknown>[] = [];
  if (main.state.field(diagramField, false)?.kept !== null) {
    effects.push(keepFenceDrawn.of(null));
  }
  if (arrow !== null && main.state.field(diagramAnchors, false)?.has(arrow)) {
    effects.push(untrackAnchor.of(arrow));
  }
  if (effects.length > 0) {
    main.dispatch({ effects });
  }
}

/** The stop the person was on, or the canvas when it went. */
function backToStop(open: OpenField): void {
  const { main, anchor, target, arrow } = open;
  const fence = fenceNow(main, anchor);
  const model = fence && modelOf(fence.body);
  const held =
    fence && model?.understood
      ? heldTarget(main, target, arrow, model, fence.bodyFrom)
      : "gone";
  const stands =
    typeof held !== "string" &&
    model !== null &&
    flowLabelOf(model, held) !== null;
  focusTarget(main, anchor, stands ? held : CANVAS);
}

/** Whether the focus went to a stop of this box, as Shift+Tab takes it. */
function onItsOwnBox(open: OpenField): boolean {
  const { main, el, anchor } = open;
  const active = main.root.activeElement;
  const host = hostOf(main, anchor);
  return (
    active !== null &&
    active !== el &&
    host !== null &&
    host.container.contains(active)
  );
}

function finish(open: OpenField, way: Way): void {
  const { main, anchor } = open;
  if (fields.get(main) !== open) {
    return;
  }
  switch (way) {
    case "enter":
      backToStop(open);
      detach(open);
      break;
    case "switch":
      main.dispatch({
        effects: setDiagramIntent.of({
          anchor,
          target: open.target,
          mode: "focus",
        }),
      });
      detach(open);
      break;
    case "blur": {
      if (onItsOwnBox(open)) {
        // As Enter does: the stop comes back after the redraw (I4).
        const intent = main.state.field(diagramIntent);
        if (intent?.mode === "rename") {
          main.dispatch({
            effects: setDiagramIntent.of({
              anchor,
              target: open.target,
              mode: "focus",
            }),
          });
        }
        detach(open);
        break;
      }
      detach(open);
      closeDiagramIntent(main, "user", "none");
      // Main took the focus with its selection in this fence (R80): the box was
      // not in its way only while the field stood there.
      if (main.hasFocus) {
        const { from, to } = main.state.selection.main;
        const fence = main.state
          .field(diagramField, false)
          ?.fences.find((f) => f.from <= from && to <= f.to);
        if (fence !== undefined) {
          main.dispatch({ effects: showFenceSource.of(fence.from) });
        }
      }
      break;
    }
    case "leave":
      detach(open);
      break;
  }
}

/** What a refused field says as long as it stands, with the way out. */
function refusalSentence(code: RefusalCode): string {
  return `${NOTICES[code]} Fix it, or press Escape in the field to drop it.`;
}

function showRefusal(open: OpenField, host: DiagramHost): void {
  if (open.refused !== null) {
    open.el.setAttribute("aria-describedby", host.statusId);
    host.holdNotice(refusalSentence(open.refused));
  }
}

function clearRefusal(open: OpenField): void {
  if (open.refused === null) {
    return;
  }
  open.refused = null;
  open.el.removeAttribute("aria-invalid");
  open.el.removeAttribute("aria-describedby");
  hostOf(open.main, open.anchor)?.holdNotice(null);
}

/**
 * The field stays with its draft, marked, and says why until the text changes. It
 * takes the focus back only from Enter, where the person is still in it: after a
 * blur the person has gone on, and may (R96).
 */
function refuse(open: OpenField, code: RefusalCode, way: Way): void {
  const { main, el, anchor, target } = open;
  el.readOnly = false;
  open.refused = code;
  el.setAttribute("aria-invalid", "true");
  const held = way === "enter" || main.root.activeElement === el;
  const intent = main.state.field(diagramIntent, false);
  const wanted = {
    anchor,
    target,
    mode: "rename" as const,
    draft: el.value,
    ...(held ? {} : { released: true as const }),
  };
  if (
    !intent ||
    intent.mode !== "rename" ||
    !sameTarget(intent.target, target) ||
    (intent.released === true) !== (wanted.released === true)
  ) {
    main.dispatch({ effects: setDiagramIntent.of(wanted) });
  }
  const host = hostOf(main, anchor);
  if (host === null) {
    main.dispatch({ effects: sayRefusal.of(code) });
    return;
  }
  showRefusal(open, host);
  if (way === "enter") {
    if (el.parentElement !== host.overlay) {
      host.overlay.append(el);
    }
    el.focus({ preventScroll: true });
  }
}

/** The op for what is typed, worked out on each read of the fence (R94). */
function opFor(
  main: EditorView,
  target: TargetKey,
  arrow: number | null,
  text: string,
): OpResolver<FlowModel, FlowOp> | null {
  if (target.kind === "canvas") {
    return null;
  }
  return (model, bodyFrom) => {
    const held = heldTarget(main, target, arrow, model, bodyFrom);
    if (typeof held === "string") {
      return { refused: held };
    }
    return held.kind === "edge"
      ? { op: "edgeLabel", edge: held.key, text }
      : { op: "rename", id: target.kind === "node" ? target.id : "", text };
  };
}

async function commitNow(open: OpenField, way: Way): Promise<void> {
  const { main, el, target } = open;
  const typed = tidyLabel(el.value);
  const op = opFor(main, target, open.arrow, typed);
  // What was typed counts against the label as it stands now, so the old name
  // typed back over a peer's rename is written. The op answers no-op on the same.
  if (op === null || !open.typed || (target.kind === "node" && typed === "")) {
    finish(open, way);
    return;
  }
  const fence = fenceNow(main, open.anchor);
  // The drawing or the text is behind, or a peer's line reads wrong just now: the
  // same words a later try will have.
  if (
    intentStale(main.state) ||
    (fence !== null && verdictFor(fence.body)?.ok === false)
  ) {
    refuse(open, "stale", way);
    return;
  }
  el.readOnly = true;
  const outcome = await runDiagramOp(main, open.anchor, flowKind, op);
  el.readOnly = false;
  if (fields.get(main) !== open) {
    return;
  }
  if (!("refused" in outcome)) {
    finish(open, way);
  } else if (outcome.refused === "gone") {
    goneNow(open);
  } else {
    refuse(open, outcome.refused, way);
  }
}

function commit(open: OpenField, way: Way): Promise<void> {
  open.checking ??= commitNow(open, way).finally(() => {
    open.checking = null;
  });
  return open.checking;
}

/** The target went: the field closes, says so and changes nothing. */
function goneNow(open: OpenField): void {
  const { main, anchor, el } = open;
  const held = main.root.activeElement === el;
  const host = hostOf(main, anchor);
  if (held) {
    focusTarget(main, anchor, CANVAS);
  } else {
    main.dispatch({
      effects: setDiagramIntent.of({ anchor, target: CANVAS, mode: "focus" }),
    });
  }
  detach(open);
  say(main, "gone", host);
}

/** A peer's edit, a new drawing or the end of a composition: is this still right? */
function reconcile(main: EditorView): void {
  const open = fields.get(main);
  if (open === undefined || open.checking !== null) {
    return;
  }
  const { el, target } = open;
  const intent = main.state.field(diagramIntent, false);
  if (intent === undefined || intent === null) {
    // Its close is on the way, and `beforeClose` has the word.
    return;
  }
  open.anchor = intent.anchor;
  if (intent.mode !== "rename" || !sameTarget(intent.target, target)) {
    // Taken elsewhere. A page that stopped taking writes takes nothing with it.
    if (writable(main.state)) {
      void commit(open, "leave");
    } else {
      const held = main.root.activeElement === el;
      detach(open);
      if (held) {
        focusTarget(main, intent.anchor, intent.target);
      }
    }
    return;
  }
  const host = hostOf(main, intent.anchor);
  if (host !== null) {
    placeField(open, host);
  }
  if (intentStale(main.state)) {
    return;
  }
  const fence = fenceAt(main.state, intent.anchor);
  const model = fence && modelNow(fence);
  if (model === null) {
    // No reading of it now: the verdict decides, and the field waits.
    return;
  }
  const held =
    fence === null
      ? target
      : heldTarget(main, target, open.arrow, model, fence.bodyFrom);
  if (held === "stale") {
    // Not said until it matters: at the commit, with a sentence.
    return;
  }
  const now = held === "gone" ? null : held;
  if (now !== null && !sameTarget(now, target)) {
    open.target = now;
    main.dispatch({
      effects: setDiagramIntent.of({ ...intent, target: now }),
    });
  }
  const label = now === null ? null : flowLabelOf(model, now);
  if (label === null) {
    if (open.composing) {
      open.goneWaits = true;
    } else {
      goneNow(open);
    }
    return;
  }
  if (!open.typed && !open.composing && label !== open.baseline) {
    const all = el.selectionStart === 0 && el.selectionEnd === el.value.length;
    open.baseline = label;
    el.value = label;
    if (all) {
      el.select();
    }
    if (host !== null) {
      placeField(open, host);
    }
    main.dispatch({
      effects: setDiagramIntent.of({
        ...intent,
        target: open.target,
        draft: label,
      }),
    });
  }
}

function makeField(
  main: EditorView,
  anchor: number,
  target: TargetKey,
  baseline: string,
  text: string,
): OpenField {
  const el = document.createElement("textarea");
  el.className = FIELD_CLASS;
  el.rows = 1;
  el.value = text;
  el.dir = "auto";
  el.setAttribute(
    "aria-label",
    target.kind === "edge" ? "Arrow label" : "Box name",
  );
  const open: OpenField = {
    main,
    el,
    target,
    anchor,
    baseline,
    typed: text !== baseline,
    composing: false,
    goneWaits: false,
    checking: null,
    arrow: null,
    refused: null,
    queued: "",
    lockTop: null,
  };
  el.addEventListener("input", () => {
    open.typed = true;
    clearRefusal(open);
    const intent = main.state.field(diagramIntent, false);
    if (intent) {
      main.dispatch({
        effects: setDiagramIntent.of({ ...intent, draft: el.value }),
      });
    }
    const host = hostOf(main, open.anchor);
    if (host !== null) {
      placeField(open, host);
    }
  });
  el.addEventListener("focus", () => {
    const intent = main.state.field(diagramIntent, false);
    if (intent?.released === true) {
      main.dispatch({
        effects: setDiagramIntent.of({
          anchor: intent.anchor,
          target: intent.target,
          mode: intent.mode,
          draft: el.value,
        }),
      });
    }
  });
  el.addEventListener("compositionstart", () => {
    open.composing = true;
    open.lockTop = el.getBoundingClientRect().top;
  });
  el.addEventListener("compositionend", () => {
    open.composing = false;
    open.lockTop = null;
    if (open.goneWaits) {
      open.goneWaits = false;
      reconcile(main);
    }
  });
  el.addEventListener("keydown", (event) => {
    onFieldKey(open, event);
  });
  el.addEventListener("blur", () => {
    // After the update that may have caused it: an element leaving the page fires
    // this in Chromium, and that is the box going away, not the person leaving.
    queueMicrotask(() => {
      onFieldBlur(open);
    });
  });
  return open;
}

function onFieldBlur(open: OpenField): void {
  const { main, el } = open;
  open.composing = false;
  open.lockTop = null;
  if (
    fields.get(main) !== open ||
    open.checking !== null ||
    !el.isConnected ||
    main.root.activeElement === el ||
    !main.dom.ownerDocument.hasFocus()
  ) {
    return;
  }
  void commit(open, "blur");
}

function onFieldKey(open: OpenField, event: KeyboardEvent): void {
  // An IME owns these keys. Safari's Enter that ends a composition arrives after
  // its compositionend, and only its key code tells.
  if (event.isComposing || open.composing || event.keyCode === 229) {
    return;
  }
  const { main, el } = open;
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    if (!event.repeat) {
      cancelLabelEditor(main);
    }
    return;
  }
  if (event.key !== "Enter") {
    return;
  }
  event.preventDefault();
  event.stopPropagation();
  if (event.shiftKey) {
    if (!el.readOnly) {
      el.setRangeText("\n", el.selectionStart, el.selectionEnd, "end");
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }
    return;
  }
  // A held Enter that opened the field is not a commit.
  if (!event.repeat) {
    void commit(open, "enter");
  }
}

/**
 * Opens the field on `target`, showing its label as it stands in the text, or
 * `initial` when given. On a page that takes no writes, or on the canvas, nothing.
 */
export function openLabelEditor(
  main: EditorView,
  anchor: number,
  target: TargetKey,
  options: { selectAll: boolean; initial?: string },
): void {
  if (!writable(main.state) || target.kind === "canvas") {
    return;
  }
  const open = fields.get(main);
  if (open !== undefined) {
    if (sameTarget(open.target, target)) {
      open.el.focus();
    } else {
      void commit(open, "switch").then(() => {
        if (!fields.has(main)) {
          openLabelEditor(
            main,
            main.state.field(diagramIntent)?.anchor ?? anchor,
            target,
            options,
          );
        }
      });
    }
    return;
  }
  const fence = fenceNow(main, anchor);
  const host = hostOf(main, anchor);
  // A fence not drawn has no box to come and no intent to hold: `settled` drops
  // one at once, and a record with no intent would go at the next key, silently.
  if (
    fence === null ||
    intentStaleAt(main, fence) ||
    !main.state.field(diagramField).fences.some((f) => f.from === fence.from)
  ) {
    say(main, "stale", host);
    return;
  }
  const model = modelNow(fence);
  const label = model && flowLabelOf(model, target);
  if (model === null || label === null || label === undefined) {
    say(main, "gone", host);
    return;
  }
  // No use typing a label the arrow cannot take on its own.
  if (target.kind === "edge") {
    const tried = compileFlowOp(model, {
      op: "edgeLabel",
      edge: target.key,
      text: label === "x" ? "y" : "x",
    });
    if ("refused" in tried && tried.refused === "group") {
      say(main, "group", host);
      return;
    }
  }
  const text = options.initial ?? label;
  const created = makeField(main, fence.from, target, label, text);
  const span =
    target.kind === "edge" ? flowEdgeSpanOf(model, target.key) : null;
  // Inside the token, so a change right at its edge stays clear of the ticket.
  const arrowEffect: StateEffect<unknown>[] = [];
  if (span !== null) {
    created.arrow = nextAnchorTicket();
    arrowEffect.push(
      trackAnchor.of({
        ticket: created.arrow,
        pos: fence.bodyFrom + span + 1,
      }),
    );
  }
  fields.set(main, created);
  syncGuards(main);
  main.dispatch({
    effects: [
      ...arrowEffect,
      setDiagramIntent.of({
        anchor: fence.from,
        target,
        mode: "rename",
        draft: text,
      }),
      keepFenceDrawn.of(fence.from),
      ...(host === null
        ? [EditorView.scrollIntoView(fence.from, { y: "center" })]
        : []),
    ],
  });
  if (host !== null) {
    mount(created, host, options.selectAll ? "select" : "fresh");
  }
}

function intentStaleAt(main: EditorView, fence: MermaidFence): boolean {
  return main.state.field(diagramField).behind.includes(fence.from);
}

/**
 * Writes the field's text as Enter does, and closes it unless it is refused. Waits
 * for a commit already running.
 */
export function commitLabelEditor(main: EditorView): Promise<void> {
  const open = fields.get(main);
  return open === undefined ? Promise.resolve() : commit(open, "enter");
}

/** Closes the field, writes nothing, and puts the focus on its stop. */
export function cancelLabelEditor(main: EditorView): void {
  const open = fields.get(main);
  if (open !== undefined && open.checking === null) {
    finish(open, "enter");
  }
}

/**
 * Runs an op that adds a box, as one undo step, and opens the field on what it
 * made with its name selected. The name is a second undo step.
 */
export async function addWithName(
  main: EditorView,
  anchor: number,
  op: Extract<FlowOp, { op: "addChild" | "addSibling" | "addBox" }>,
): Promise<void> {
  // The intent follows the fence through what a peer writes during the check.
  const before = main.state.field(diagramIntent);
  if (before === null || before.anchor !== anchor) {
    main.dispatch({
      effects: setDiagramIntent.of({ anchor, target: CANVAS, mode: "focus" }),
    });
  }
  const outcome = await runDiagramOp(main, anchor, flowKind, op);
  const place = main.state.field(diagramIntent, false)?.anchor ?? anchor;
  if ("refused" in outcome) {
    say(main, outcome.refused, hostOf(main, place));
  } else if (outcome.applied && outcome.created !== undefined) {
    openLabelEditor(main, place, outcome.created, { selectAll: true });
  }
}

/** A close that did not come from the field. */
function onClosing(
  main: EditorView,
  reason: IntentCloseReason,
  intent: DiagramIntent,
): void {
  const open = fields.get(main);
  if (open === undefined) {
    return;
  }
  open.anchor = intent.anchor;
  if (reason === "swap" || reason === "destroy") {
    fields.delete(main);
    open.el.remove();
    syncGuards(main);
    return;
  }
  if (open.checking !== null) {
    return;
  }
  if (reason === "gone") {
    detach(open);
    main.dispatch({ effects: sayRefusal.of("gone") });
    return;
  }
  if (reason === "raw") {
    void commitForRaw(open);
    return;
  }
  void commit(open, "leave");
}

/** A refused draft waiting for Raw to go off (R97). */
interface RawDraft {
  target: TargetKey;
  arrow: number | null;
  /** The ticket holding the fence's place. */
  place: number;
  baseline: string;
  draft: string;
  code: RefusalCode;
}

const rawDrafts = new WeakMap<EditorView, RawDraft>();

const rawNoteShown = StateEffect.define<string | null>();

/** Outside the layer, as Raw takes the layer and its panels with it. */
const rawNote = StateField.define<string | null>({
  create: () => null,
  update: (value, tr) => {
    let next = value;
    for (const effect of tr.effects) {
      if (effect.is(rawNoteShown)) {
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
            dom.className = "cm-diagram-raw-note";
            dom.setAttribute("role", "status");
            dom.textContent = sentence;
            return { dom };
          },
    ),
});

const rawNoteTheme = EditorView.baseTheme({
  ".cm-diagram-raw-note": {
    padding: "0.25rem 0.75rem",
    color: "var(--color-slate-700)",
    backgroundColor: "var(--color-slate-100)",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
  },
  "&dark .cm-diagram-raw-note": {
    color: "var(--color-slate-200)",
    backgroundColor: "var(--color-slate-800)",
  },
});

/**
 * Raw took the box and the field with it, so all that is left is the text. A text
 * refused here comes back in its field when Raw goes off, and until then a line
 * outside the layer says so (R97).
 */
async function commitForRaw(open: OpenField): Promise<void> {
  const { main, el, target, anchor, arrow } = open;
  fields.delete(main);
  const typed = tidyLabel(el.value);
  el.remove();
  syncGuards(main);
  const op = opFor(main, target, arrow, typed);
  const refused =
    op === null || !open.typed || (target.kind === "node" && typed === "")
      ? null
      : await runDiagramOp(main, anchor, flowKind, op);
  if (refused === null || !("refused" in refused) || !main.dom.isConnected) {
    if (arrow !== null && main.state.field(diagramAnchors, false)?.has(arrow)) {
      main.dispatch({ effects: untrackAnchor.of(arrow) });
    }
    return;
  }
  const place = nextAnchorTicket();
  const sentence = `${NOTICES[refused.refused]} "${el.value}" comes back in its field when Raw is off.`;
  rawDrafts.set(main, {
    target,
    arrow,
    place,
    baseline: open.baseline,
    draft: el.value,
    code: refused.refused,
  });
  syncGuards(main);
  main.dispatch({
    effects: [
      ...(main.state.field(rawNote, false) === undefined
        ? [StateEffect.appendConfig.of([rawNote, rawNoteTheme])]
        : []),
      trackAnchor.of({ ticket: place, pos: anchor + 1 }),
    ],
  });
  main.dispatch({
    effects: [rawNoteShown.of(sentence), EditorView.announce.of(sentence)],
  });
}

/** Raw is off: the refused draft is back in its field, once its fence is drawn. */
function restoreRawDraft(main: EditorView): void {
  const raw = rawDrafts.get(main);
  if (raw === undefined || fields.has(main)) {
    return;
  }
  const at = main.state.field(diagramAnchors, false)?.get(raw.place);
  const fence = at === undefined ? null : fenceNow(main, at);
  const model = fence && modelNow(fence);
  const end = () => {
    rawDrafts.delete(main);
    syncGuards(main);
    main.dispatch({
      effects: [
        untrackAnchor.of(raw.place),
        ...(raw.arrow === null ? [] : [untrackAnchor.of(raw.arrow)]),
        ...(main.state.field(rawNote, false) === undefined
          ? []
          : [rawNoteShown.of(null)]),
      ],
    });
  };
  const held =
    fence === null || model === null
      ? "gone"
      : heldTarget(main, raw.target, raw.arrow, model, fence.bodyFrom);
  if (fence === null || model === null || typeof held === "string") {
    end();
    say(main, "gone", null);
    return;
  }
  if (
    !main.state.field(diagramField).fences.some((f) => f.from === fence.from)
  ) {
    return;
  }
  end();
  openLabelEditor(main, fence.from, held, {
    selectAll: false,
    initial: raw.draft,
  });
  const open = fields.get(main);
  const host = hostOf(main, fence.from);
  if (open !== undefined) {
    open.refused = raw.code;
    open.el.setAttribute("aria-invalid", "true");
    if (host !== null) {
      showRefusal(open, host);
    }
  }
}

/** Whether leaving now would lose what the field holds. */
function hasDraft(open: OpenField): boolean {
  const typed = tidyLabel(open.el.value);
  return (
    open.typed &&
    typed !== tidyLabel(open.baseline) &&
    !(open.target.kind === "node" && typed === "")
  );
}

const guards = new WeakMap<EditorView, () => void>();

/** The listeners that hold a page while a field is open, and only then (R95, R98). */
function syncGuards(main: EditorView): void {
  const wanted = fields.has(main) || rawDrafts.has(main);
  const stop = guards.get(main);
  if (wanted && stop === undefined) {
    guards.set(main, armGuards(main));
  } else if (!wanted && stop !== undefined) {
    stop();
    guards.delete(main);
  }
}

function armGuards(main: EditorView): () => void {
  let replaying = false;
  let restoring = false;
  const historyAt = historyIndex();

  const onClick = (event: MouseEvent) => {
    const target = event.target;
    if (replaying || !main.dom.isConnected || !(target instanceof Element)) {
      return;
    }
    if (target.closest(".cm-diagram") !== null && main.dom.contains(target)) {
      return;
    }
    const link = target.closest<HTMLAnchorElement>("a[href]");
    const leaving =
      (link !== null || target.closest("[data-leaves-editor]") !== null) &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.shiftKey &&
      event.button === 0 &&
      link?.target !== "_blank";
    const open = fields.get(main);
    if (open === undefined) {
      // A refused draft under Raw: its line stands, and the page stays.
      if (leaving && rawDrafts.has(main)) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
      return;
    }
    const running = open.checking !== null;
    if (!running && !(leaving && hasDraft(open))) {
      return;
    }
    event.preventDefault();
    event.stopImmediatePropagation();
    const replay = new MouseEvent("click", event);
    void commit(open, "blur").then(() => {
      if (!leaving || fields.get(main) !== open) {
        replaying = true;
        target.dispatchEvent(replay);
        replaying = false;
      }
    });
  };

  const onPopstate = (event: PopStateEvent) => {
    if (restoring) {
      restoring = false;
      event.stopImmediatePropagation();
      return;
    }
    if (!main.dom.isConnected) {
      return;
    }
    const undo = () => {
      restoring = true;
      const now = historyIndex();
      if (historyAt !== null && now !== null && now !== historyAt) {
        window.history.go(historyAt - now);
      } else {
        window.history.forward();
      }
    };
    const open = fields.get(main);
    if (open === undefined) {
      if (rawDrafts.has(main)) {
        event.stopImmediatePropagation();
        undo();
      }
      return;
    }
    if (open.checking === null && !hasDraft(open)) {
      return;
    }
    event.stopImmediatePropagation();
    const state: unknown = event.state;
    void commit(open, "blur").then(() => {
      if (fields.get(main) === open) {
        undo();
      } else {
        window.dispatchEvent(new PopStateEvent("popstate", { state }));
      }
    });
  };

  const onUnload = (event: BeforeUnloadEvent) => {
    const open = fields.get(main);
    if (
      main.dom.isConnected &&
      (rawDrafts.has(main) || (open !== undefined && hasDraft(open)))
    ) {
      event.preventDefault();
    }
  };

  const onKey = (event: KeyboardEvent) => {
    const open = fields.get(main);
    if (
      open === undefined ||
      open.el.isConnected ||
      !main.dom.isConnected ||
      event.metaKey ||
      event.ctrlKey ||
      event.altKey
    ) {
      return;
    }
    const active = main.root.activeElement;
    if (active !== null && active !== main.dom.ownerDocument.body) {
      return;
    }
    const printable = event.key.length === 1;
    const { key } = event;
    if (
      !printable &&
      !["Backspace", "Delete", "Enter", "Escape", "Process"].includes(key)
    ) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (key === "Escape") {
      cancelLabelEditor(main);
      return;
    }
    if (key === "Enter") {
      void commit(open, "enter");
      return;
    }
    if (printable) {
      open.queued += key;
    }
    main.dispatch({
      effects: EditorView.scrollIntoView(open.anchor, { y: "center" }),
    });
  };

  // A composition goes on where it began: an element taken out of the page ends
  // it and loses its last letter, so the page waits for its end (R98).
  const onScroll = (event: Event) => {
    const open = fields.get(main);
    if (open?.lockTop === null || open === undefined || !open.el.isConnected) {
      return;
    }
    const moved = open.el.getBoundingClientRect().top - open.lockTop;
    if (Math.abs(moved) < 0.5) {
      return;
    }
    const scroller =
      event.target instanceof Element
        ? event.target
        : main.dom.ownerDocument.scrollingElement;
    if (scroller !== null) {
      scroller.scrollTop += moved;
    }
  };

  window.addEventListener("click", onClick, true);
  backGates.add(onPopstate);
  window.addEventListener("beforeunload", onUnload);
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("scroll", onScroll, true);
  return () => {
    window.removeEventListener("click", onClick, true);
    backGates.delete(onPopstate);
    window.removeEventListener("beforeunload", onUnload);
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("scroll", onScroll, true);
  };
}

/** The router's place in the history, which it keeps in the entry's state. */
function historyIndex(): number | null {
  const state = window.history.state as { idx?: unknown } | null;
  return typeof state?.idx === "number" ? state.idx : null;
}

registerStopAction("rename", (main, anchor, target) => {
  openLabelEditor(main, anchor, target, { selectAll: true });
});

registerClickGate((main, target, go) => {
  const open = fields.get(main);
  if (open === undefined) {
    go();
  } else if (sameTarget(open.target, target)) {
    open.el.focus();
  } else {
    void commit(open, "switch").then(() => {
      if (!fields.has(main)) {
        go();
      }
    });
  }
});

registerIntentReopen("rename", (main, intent, host) => {
  let open = fields.get(main);
  if (open === undefined) {
    if (intent.target.kind === "canvas") {
      const active = main.root.activeElement;
      if (active === null || active === main.dom.ownerDocument.body) {
        host.container.focus({ preventScroll: true });
      }
      return;
    }
    const fence = fenceAt(main.state, intent.anchor);
    const model = fence && modelNow(fence);
    const label = (model && flowLabelOf(model, intent.target)) ?? "";
    open = makeField(
      main,
      intent.anchor,
      intent.target,
      label,
      intent.draft ?? label,
    );
    fields.set(main, open);
  }
  mount(open, host, "back");
});

/** Whether the selection is one box's label in the fence it stands in. */
function selectedLabel(main: EditorView): {
  fence: MermaidFence;
  id: string;
} | null {
  const { state } = main;
  const { selection } = state;
  const range = selection.main;
  if (selection.ranges.length > 1 || range.empty) {
    return null;
  }
  const fence = fenceAt(state, range.from);
  if (fence === null || !fence.topLevel || range.to > fence.to) {
    return null;
  }
  const model = modelNow(fence);
  if (model === null) {
    return null;
  }
  const from = range.from - fence.bodyFrom;
  const to = range.to - fence.bodyFrom;
  for (const id of model.nodeOrder) {
    const label = model.nodes.get(id)?.defining?.shape?.label;
    if (label?.from === from && label.to === to) {
      return { fence, id };
    }
  }
  return null;
}

/**
 * The carry from R52: an inserted starter stands with its caret inside, and only
 * now does it draw. Only with a settled good verdict, so a person who typed over
 * the selection meanwhile keeps a fence of text under their caret.
 */
function openOnStarter(main: EditorView): boolean {
  if (
    fields.has(main) ||
    main.state.field(diagramIntent) !== null ||
    !writable(main.state) ||
    !main.hasFocus ||
    main.composing
  ) {
    return false;
  }
  const found = selectedLabel(main);
  if (found === null || verdictFor(found.fence.body)?.ok !== true) {
    return false;
  }
  const { from } = found.fence;
  if (!main.state.field(diagramField).fences.some((f) => f.from === from)) {
    main.dispatch({ effects: drawUnderCaret.of(from) });
  }
  // The field takes the focus before main could: with its caret in a drawn fence
  // the next key is refused, and main focused would show the fence as text.
  openLabelEditor(
    main,
    from,
    { kind: "node", id: found.id },
    {
      selectAll: true,
    },
  );
  return fields.has(main);
}

/** Views whose close hook is in: it outlives the plugin, as Raw takes that first. */
const hooked = new WeakSet<EditorView>();

export const labelEditorPlugin = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;
    readonly unsubscribe: (() => void)[];
    /** An input left a selection, so a settling verdict may open on it. */
    starter = false;

    constructor(view: EditorView) {
      this.view = view;
      this.unsubscribe = [
        onDiagramDrawn(view, (host) => {
          const open = fields.get(view);
          if (open !== undefined && hostOf(view, open.anchor) === host) {
            reconcile(view);
          }
        }),
      ];
      // This chunk loads with the first fence, so on a page that had none the
      // starter's own transaction came before this plugin existed.
      this.starter = !view.state.selection.main.empty;
      if (this.starter) {
        queueMicrotask(() => {
          this.tryStarter();
        });
      }
      if (rawDrafts.has(view)) {
        queueMicrotask(() => {
          restoreRawDraft(view);
        });
      }
      if (!hooked.has(view)) {
        hooked.add(view);
        beforeClose(view, (reason, intent) => {
          onClosing(view, reason, intent);
        });
      }
    }

    update(update: ViewUpdate): void {
      const { view } = this;
      const open = fields.get(view);
      if (rawDrafts.has(view) && update.docChanged) {
        queueMicrotask(() => {
          restoreRawDraft(view);
        });
      }
      if (
        open !== undefined &&
        (update.docChanged ||
          update.geometryChanged ||
          update.startState.field(diagramIntent) !==
            update.state.field(diagramIntent))
      ) {
        if (update.state.field(diagramIntent) === null && update.docChanged) {
          open.anchor = update.changes.mapPos(open.anchor, 1);
        }
        queueMicrotask(() => {
          reconcile(view);
        });
      }
      let check = false;
      for (const tr of update.transactions) {
        if (
          tr.docChanged &&
          tr.isUserEvent("input") &&
          !tr.isUserEvent("input.diagram")
        ) {
          this.starter = !tr.newSelection.main.empty;
          check = this.starter;
        } else if (
          tr.selection !== undefined ||
          (tr.docChanged && tr.annotation(Transaction.userEvent) !== undefined)
        ) {
          this.starter = false;
        }
        check ||= this.starter && tr.effects.some((e) => e.is(verdictArrived));
      }
      // A menu that made the insert still held the focus when it landed.
      check ||= this.starter && update.focusChanged;
      if (check) {
        queueMicrotask(() => {
          this.tryStarter();
        });
      }
    }

    tryStarter(): void {
      if (this.starter && openOnStarter(this.view)) {
        this.starter = false;
      }
    }

    destroy(): void {
      for (const stop of this.unsubscribe) {
        stop();
      }
    }
  },
);
