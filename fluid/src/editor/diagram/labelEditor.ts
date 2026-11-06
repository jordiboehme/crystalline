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
 * - `setState` and the view going away drop it: another document, no box.
 * - A refusal keeps it open with its draft and says why. The way out is Escape.
 *
 * With a peer: a body edit keeps the field, and the fence stays drawn while it is
 * open whatever mermaid says of a half-typed line (`keepFenceDrawn`). Two people
 * renaming a box is last writer wins, except that a field nobody typed in shows the
 * peer's new name, so an Enter there cannot put the old one back.
 */

import { forceParsing, syntaxTree } from "@codemirror/language";
import { Transaction } from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin } from "@codemirror/view";

import { writable } from "../elementEdit";
import { flowKind, runDiagramOp } from "./apply";
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
import { flowLabelOf } from "./flowchart/hit";
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
}

const fields = new WeakMap<EditorView, OpenField>();

const sameTarget = (a: TargetKey, b: TargetKey) =>
  JSON.stringify(a) === JSON.stringify(b);

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
  const height = Math.min(
    Math.max(el.scrollHeight, place.height, MIN_HEIGHT),
    MAX_HEIGHT,
  );
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
    active === main.contentDOM ||
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
  if (how === "back") {
    // A box coming back: the focus went with the old one, or went somewhere.
    if (focusIsFree(open, host)) {
      el.focus({ preventScroll: true });
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
  fields.delete(open.main);
  open.el.remove();
  if (open.main.state.field(diagramField, false)?.kept !== null) {
    open.main.dispatch({ effects: keepFenceDrawn.of(null) });
  }
}

/** The stop the person was on, or the canvas when it went. */
function backToStop(open: OpenField): void {
  const { main, anchor, target } = open;
  const fence = fenceNow(main, anchor);
  const model = fence && modelOf(fence.body);
  const stands = model?.understood && flowLabelOf(model, target) !== null;
  focusTarget(main, anchor, stands ? target : CANVAS);
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

/** The field stays with its draft and says why. */
function refuse(open: OpenField, code: RefusalCode): void {
  const { main, el, anchor, target } = open;
  el.readOnly = false;
  const intent = main.state.field(diagramIntent, false);
  if (
    intent === undefined ||
    intent === null ||
    intent.mode !== "rename" ||
    !sameTarget(intent.target, target)
  ) {
    main.dispatch({
      effects: setDiagramIntent.of({
        anchor,
        target,
        mode: "rename",
        draft: el.value,
      }),
    });
  }
  const host = hostOf(main, anchor);
  if (host === null) {
    main.dispatch({ effects: sayRefusal.of(code) });
    return;
  }
  host.notice(code);
  if (focusIsFree(open, host)) {
    if (el.parentElement !== host.overlay) {
      host.overlay.append(el);
    }
    el.focus({ preventScroll: true });
  }
}

function opFor(target: TargetKey, text: string): FlowOp | null {
  if (target.kind === "node") {
    return { op: "rename", id: target.id, text };
  }
  return target.kind === "edge"
    ? { op: "edgeLabel", edge: target.key, text }
    : null;
}

async function commitNow(open: OpenField, way: Way): Promise<void> {
  const { main, el, target } = open;
  const typed = tidyLabel(el.value);
  const op = opFor(target, typed);
  if (
    op === null ||
    typed === tidyLabel(open.baseline) ||
    (target.kind === "node" && typed === "")
  ) {
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
    refuse(open, "stale");
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
    refuse(open, outcome.refused);
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
  if (host === null) {
    main.dispatch({ effects: sayRefusal.of("gone") });
  } else {
    host.notice("gone");
  }
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
  const label = flowLabelOf(model, target);
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
      effects: setDiagramIntent.of({ ...intent, draft: label }),
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
  };
  el.addEventListener("input", () => {
    open.typed = true;
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
  el.addEventListener("compositionstart", () => {
    open.composing = true;
  });
  el.addEventListener("compositionend", () => {
    open.composing = false;
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
  if (fence === null || intentStaleAt(main, fence)) {
    host?.notice("stale");
    return;
  }
  const model = modelNow(fence);
  const label = model && flowLabelOf(model, target);
  if (model === null || label === null || label === undefined) {
    host?.notice("gone");
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
      host?.notice("group");
      return;
    }
  }
  const text = options.initial ?? label;
  const created = makeField(main, fence.from, target, label, text);
  fields.set(main, created);
  main.dispatch({
    effects: [
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
    const host = hostOf(main, place);
    if (host === null) {
      main.dispatch({ effects: sayRefusal.of(outcome.refused) });
    } else {
      host.notice(outcome.refused);
    }
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

/**
 * Raw took the box and the field with it, so all that is left is the text. A text
 * refused here has no place to stay, so the page says why.
 */
async function commitForRaw(open: OpenField): Promise<void> {
  const { main, el, target, anchor } = open;
  fields.delete(main);
  const typed = tidyLabel(el.value);
  el.remove();
  const op = opFor(target, typed);
  if (
    op === null ||
    typed === tidyLabel(open.baseline) ||
    (target.kind === "node" && typed === "")
  ) {
    return;
  }
  const outcome = await runDiagramOp(main, anchor, flowKind, op);
  if ("refused" in outcome && main.dom.isConnected) {
    main.dispatch({
      effects: EditorView.announce.of(NOTICES[outcome.refused]),
    });
  }
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
