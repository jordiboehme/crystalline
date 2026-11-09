/**
 * What the menus of a drawn flowchart offer, and connect mode. Every item is a
 * call of `runDiagramOp`, so one undo step and one verify, or of a thing that makes
 * those calls (the rename field, adding a box, removing the fence). The menus of a
 * sequence diagram open, close and follow a peer here too, and connect mode adds a
 * message there: `sequence/actions.ts` says what is in those menus.
 *
 * What an item acts on is read when it runs, not when the menu opened: a box by its
 * id, an arrow by its place in the text (R94). With duplicates in play and that
 * place gone it answers "stale", where a retry is true: the menu opens again on what
 * stands now. A peer's edit elsewhere keeps the menu open, and what it shows follows.
 * A peer taking the box or arrow closes the menu with the "gone" notice.
 *
 * While a menu or connect mode is open its fence stays drawn, as for the rename
 * field: a peer's half-typed line must not take away what the person is using, and
 * nothing is written while the verdict is against the body.
 *
 * Leaving. Every close of the intent ends both here (`onIntentClosed`). A menu's own
 * close says how it went (`menu.ts`), and connect mode ends on Escape, on a click on
 * the canvas, on its Cancel button, on a right click, on the focus leaving the
 * diagram or on a press in the text. It writes nothing unless a box was chosen.
 */

import { EditorView, ViewPlugin } from "@codemirror/view";
import type { ViewUpdate } from "@codemirror/view";

import { fenceToAct, placeNow } from "../act";
import type { ApplyOutcome } from "../apply";
import { flowKind, runDiagramOp, seqKind } from "../apply";
import type { RefusalCode } from "../edits";
import { reasonFor } from "../edits";
import { fenceAt } from "../fence";
import { diagramField, keepFenceDrawn } from "../field";
import {
  backOnStop,
  clearStopLook,
  deleteFenceOf,
  focusTarget,
  prepareStop,
  registerConnectHooks,
  registerStopAction,
  sayAt,
  targetElement,
} from "../focus";
import type { FlowTarget, IntentCloseReason, TargetKey } from "../intent";
import type { LineHold } from "../held";
import {
  holdLine,
  holdsLine,
  letGoOfLine,
  resolveHeldTarget,
  ticketless,
} from "../held";
import {
  closeDiagramIntent,
  diagramIntent,
  intentStale,
  onIntentClosed,
  registerIntentReopen,
  setDiagramIntent,
} from "../intent";
import {
  addWithName,
  fenceNow,
  modelNow,
  openLabelEditor,
} from "../labelEditor";
import type { MenuClose, MenuHandle, MenuItem } from "../menu";
import { openMenu } from "../menu";
import type { DiagramModel } from "../kinds";
import {
  connectIdOf,
  connectStopOf,
  connectable,
  labelOfTarget,
} from "../kinds";
import { SEQ_CONNECT_PROMPT, seqMenu } from "../sequence/actions";
import type { Verdict } from "../verdict";
import type { DiagramHost } from "../widget";
import { hostOf } from "../widget";
import { COLOUR_KEYS, readDirection } from "./checked";
import type { ColourPreset } from "./labels";
import { COLOUR_PRESETS, SHAPES } from "./labels";
import type { Arrow, EdgeKey, FlowModel } from "./model";
import type { FlowOp } from "./ops";
import { compileFlowOp } from "./ops";
import { edgeByKey } from "./parse";

const CANVAS: TargetKey = { kind: "canvas" };
const PROMPT = "Choose the box to connect to. Press Escape to cancel.";
const CONNECT_CLASS = "cm-diagram-connect";
const CONNECTING_CLASS = "cm-diagram-connecting";
/** Off the corner of the box, for a menu on a canvas opened by a key. */
const CANVAS_INSET = 12;

/** The arrow or message an open menu or its running item holds. */
const holds = new WeakMap<EditorView, LineHold>();

function holdFor(
  main: EditorView,
  anchor: number,
  target: Extract<TargetKey, { kind: "edge" | "message" }>,
  model: DiagramModel,
): LineHold {
  const had = holds.get(main);
  if (had !== undefined && holdsLine(had, target)) {
    return had;
  }
  const hold =
    holdLine(main, model, target, fenceAt(main.state, anchor)?.bodyFrom ?? 0) ??
    ticketless(target);
  holds.set(main, hold);
  return hold;
}

async function runOp(
  main: EditorView,
  anchor: number,
  hold: LineHold | null,
  op: FlowOp | ((edge: EdgeKey) => FlowOp),
): Promise<void> {
  try {
    const place = placeNow(main, anchor);
    if (fenceToAct(main, place) === null) {
      return;
    }
    const outcome = await runDiagramOp(
      main,
      place,
      flowKind,
      (model, bodyFrom): FlowOp | { refused: RefusalCode } => {
        if (typeof op !== "function") {
          return op;
        }
        if (hold === null || !("key" in hold)) {
          return { refused: "gone" };
        }
        const edge = resolveHeldTarget(
          main,
          { kind: "edge", key: hold.key },
          hold,
          model,
          bodyFrom,
        );
        if (typeof edge === "string") {
          return { refused: edge };
        }
        return edge.kind === "edge" ? op(edge.key) : { refused: "gone" };
      },
    );
    if ("refused" in outcome) {
      sayAt(main, placeNow(main, place), outcome.refused);
    }
  } finally {
    if (hold !== null) {
      letGoOfLine(main, hold);
    }
  }
}

/** The preset a box's `style` lines make, or null with none or some other colour. */
function colourOf(model: FlowModel, id: string): ColourPreset | "none" | null {
  const values = new Map<string, string>();
  for (const line of model.directives) {
    if (line.kind === "style" && line.targets[0]?.id === id) {
      for (const prop of line.props) {
        const key = prop.key.toLowerCase();
        if ((COLOUR_KEYS as readonly string[]).includes(key)) {
          values.set(
            key,
            model.body.slice(prop.value.from, prop.value.to).toLowerCase(),
          );
        }
      }
    }
  }
  if (values.size === 0) {
    // A class can draw it coloured, so "No colour" would not be true.
    const classed =
      model.nodes.get(id)?.refs.some((ref) => ref.classSpan !== null) ===
        true ||
      model.directives.some(
        (line) =>
          (line.kind === "class" &&
            line.targets.some((target) => target.id === id)) ||
          (line.kind === "classDef" &&
            line.targets.some((target) => target.id === "default")),
      );
    return classed ? null : "none";
  }
  for (const [name, preset] of Object.entries(COLOUR_PRESETS)) {
    if (COLOUR_KEYS.every((key) => values.get(key) === preset[key])) {
      return name as ColourPreset;
    }
  }
  return null;
}

function boxMenu(
  main: EditorView,
  anchor: number,
  id: string,
  model: FlowModel,
): MenuItem[] {
  const op = (flowOp: FlowOp) => () => {
    void runOp(main, anchor, null, flowOp);
  };
  const shape = model.nodes.get(id)?.defining?.shape;
  const current = shape === undefined || shape === null ? "rect" : shape.name;
  const colour = colourOf(model, id);
  return [
    {
      label: "Rename",
      run: () => {
        openLabelEditor(
          main,
          placeNow(main, anchor),
          { kind: "node", id },
          {
            selectAll: true,
          },
        );
      },
    },
    {
      label: "Add child",
      run: () => {
        void addWithName(main, placeNow(main, anchor), {
          op: "addChild",
          id,
        });
      },
    },
    {
      label: "Add sibling",
      run: () => {
        void addWithName(main, placeNow(main, anchor), {
          op: "addSibling",
          id,
        });
      },
    },
    {
      label: "Connect to...",
      run: () => {
        startConnect(main, placeNow(main, anchor), id);
      },
    },
    {
      label: "Shape",
      submenu: SHAPES.map((entry) => ({
        label: entry.label,
        checked: entry.name === current,
        run: op({ op: "shape", id, shape: entry.name }),
      })),
    },
    {
      label: "Colour",
      submenu: [
        {
          label: "No colour",
          checked: colour === "none",
          run: op({ op: "colour", id, colour: null }),
        },
        ...Object.entries(COLOUR_PRESETS).map(([name, preset]) => ({
          label: preset.label,
          checked: colour === name,
          run: op({ op: "colour", id, colour: name as ColourPreset }),
        })),
      ],
    },
    { label: "Delete", run: op({ op: "deleteNode", id }) },
  ];
}

/** Why `op` could not be made, for an item that says so before it is chosen. */
function blockedBy(model: FlowModel, op: FlowOp): Partial<MenuItem> {
  const result = compileFlowOp(model, op);
  return "refused" in result
    ? { disabled: true, description: reasonFor(result.refused) }
    : {};
}

function headsOf(arrow: Arrow | undefined): "arrow" | "none" | "both" | null {
  if (arrow?.end === "arrow") {
    return arrow.start === "none"
      ? "arrow"
      : arrow.start === "arrow"
        ? "both"
        : null;
  }
  return arrow?.end === "none" && arrow.start === "none" ? "none" : null;
}

function arrowMenu(
  main: EditorView,
  anchor: number,
  key: EdgeKey,
  model: FlowModel,
): MenuItem[] {
  const hold = holdFor(main, anchor, { kind: "edge", key }, model);
  const op = (flowOp: (edge: EdgeKey) => FlowOp) => () => {
    holds.delete(main);
    void runOp(main, anchor, hold, flowOp);
  };
  const edge = edgeByKey(model, key);
  const arrow =
    edge === null
      ? undefined
      : model.chains[edge.chain]?.links[edge.link]?.arrow;
  const heads = headsOf(arrow);
  return [
    {
      label: "Edit label",
      run: () => {
        holds.delete(main);
        editLabel(main, anchor, hold, key);
      },
    },
    {
      label: "Line",
      submenu: (
        [
          ["Solid", "solid"],
          ["Dotted", "dotted"],
          ["Thick", "thick"],
        ] as const
      ).map(([label, line]) => ({
        label,
        checked: arrow?.line === line,
        run: op((at) => ({ op: "lineStyle", edge: at, line })),
      })),
    },
    {
      label: "Arrowhead",
      submenu: (
        [
          ["Arrow", "arrow"],
          ["None", "none"],
          ["Both ends", "both"],
        ] as const
      ).map(([label, head]) => ({
        label,
        checked: heads === head,
        run: op((at) => ({ op: "arrowhead", edge: at, head })),
      })),
    },
    {
      label: "Reverse",
      run: op((at) => ({ op: "reverse", edge: at })),
      ...blockedBy(model, { op: "reverse", edge: key }),
    },
    {
      label: "Delete",
      run: op((at) => ({ op: "deleteEdge", edge: at })),
      ...blockedBy(model, { op: "deleteEdge", edge: key }),
    },
  ];
}

/** The field opens on the arrow the menu was opened on, wherever it is now. */
function editLabel(
  main: EditorView,
  anchor: number,
  hold: LineHold,
  key: EdgeKey,
): void {
  const place = placeNow(main, anchor);
  const fence = fenceNow(main, place);
  const model = fence && modelNow(fence);
  const edge =
    fence === null || model === null
      ? "gone"
      : resolveHeldTarget(
          main,
          { kind: "edge", key },
          hold,
          model,
          fence.bodyFrom,
        );
  letGoOfLine(main, hold);
  if (typeof edge === "string") {
    sayAt(main, place, edge);
    return;
  }
  openLabelEditor(main, place, edge, { selectAll: true });
}

function diagramMenu(
  main: EditorView,
  anchor: number,
  model: FlowModel,
): MenuItem[] {
  const direction = readDirection(model.direction?.value);
  return [
    {
      label: "Add box",
      run: () => {
        void addWithName(main, placeNow(main, anchor), { op: "addBox" });
      },
    },
    {
      label: "Direction",
      submenu: (
        [
          ["Top to bottom", "TD", "TB"],
          ["Left to right", "LR", "LR"],
        ] as const
      ).map(([label, value, read]) => ({
        label,
        checked: direction === read,
        run: () => {
          void runOp(main, anchor, null, { op: "direction", value });
        },
      })),
    },
    {
      label: "Delete diagram",
      run: () => {
        deleteFenceOf(main, placeNow(main, anchor));
      },
    },
  ];
}

/** The menu of `target`: its accessible name and its items. */
export function flowMenu(
  main: EditorView,
  anchor: number,
  target: FlowTarget,
  model: FlowModel,
): { name: string; items: MenuItem[] } {
  return menuOf(main, anchor, target, model);
}

/** The menu of `target` on a diagram of either kind. */
function menuOf(
  main: EditorView,
  anchor: number,
  target: TargetKey,
  model: DiagramModel,
): { name: string; items: MenuItem[] } {
  if (model.kind === "sequence") {
    return seqMenu(main, anchor, target, model, {
      connect: (at, name) => {
        startConnect(main, at, name);
      },
      deleteDiagram: (at) => {
        deleteFenceOf(main, at);
      },
      hold: (message) => holdFor(main, anchor, message, model),
      release: () => {
        holds.delete(main);
      },
    });
  }
  switch (target.kind) {
    case "node":
      return {
        name: "Box actions",
        items: boxMenu(main, anchor, target.id, model),
      };
    case "edge":
      return {
        name: "Arrow actions",
        items: arrowMenu(main, anchor, target.key, model),
      };
    default:
      return {
        name: "Diagram actions",
        items: diagramMenu(main, anchor, model),
      };
  }
}

/** This fence's hold, not one a rename field on another fence has taken since. */
function releaseFence(main: EditorView, anchor: number): void {
  if (main.state.field(diagramField, false)?.kept === anchor) {
    main.dispatch({ effects: keepFenceDrawn.of(null) });
  }
}

interface OpenMenu {
  main: EditorView;
  handle: MenuHandle | null;
  anchor: number;
  target: TargetKey;
  /** The body its items were last worked out on. */
  body: string;
  /** Its box or arrow went: the focus goes to the canvas. */
  gone: boolean;
  /** Where the focus goes back to, worked out when the menu closes. */
  back: Element | null;
}

const menus = new WeakMap<EditorView, OpenMenu>();

/** The swallowed click of a press that only dismissed a menu. */
function dismissingClick(main: EditorView): void {
  const swallow = (event: MouseEvent) => {
    const el = event.target;
    if (el instanceof Element && main.dom.contains(el)) {
      if (el.closest(".cm-diagram") !== null) {
        event.stopPropagation();
        event.preventDefault();
      }
    }
  };
  const stop = () => {
    window.removeEventListener("click", swallow, { capture: true });
    window.removeEventListener("contextmenu", stop, { capture: true });
    window.removeEventListener("mousedown", stop, { capture: true });
  };
  window.addEventListener("click", swallow, { capture: true, once: true });
  // A right button's press has no click, and on some systems no mouse up.
  window.addEventListener("contextmenu", stop, { capture: true, once: true });
  // Nor has a press that started a drag: the next press is a click of its own.
  window.addEventListener("mousedown", stop, { capture: true, once: true });
  window.addEventListener(
    "mouseup",
    () => {
      setTimeout(stop, 0);
    },
    { capture: true, once: true },
  );
}

/** The menu closed, by any route: what it held goes, and the stop takes over. */
function menuClosed(open: OpenMenu, reason: MenuClose): void {
  const { main, anchor } = open;
  menus.delete(main);
  const hold = holds.get(main);
  const intent = main.state.field(diagramIntent, false);
  const ours = intent?.mode === "menu" && intent.anchor === anchor;
  let target: TargetKey = CANVAS;
  if (ours && !open.gone && open.target.kind !== "canvas") {
    const fence = fenceAt(main.state, anchor);
    const model = fence && modelNow(fence);
    if (fence !== null && model !== null) {
      const held = resolveHeldTarget(
        main,
        open.target,
        hold ?? null,
        model,
        fence.bodyFrom,
      );
      // Not another arrow of the pair: the ring would stand on what a key deletes.
      if (typeof held !== "string" && labelOfTarget(model, held) !== null) {
        target = held;
      }
    }
  }
  // An item's own run still needs its arrow, and lets go when it is done.
  if (hold !== undefined && reason !== "item") {
    holds.delete(main);
    letGoOfLine(main, hold);
  }
  if (!main.dom.isConnected) {
    return;
  }
  releaseFence(main, anchor);
  if (!ours) {
    return;
  }
  main.dispatch({
    effects: setDiagramIntent.of({ anchor, target, mode: "focus" }),
  });
  const host = hostOf(main, anchor);
  open.back = host === null ? null : prepareStop(host, target);
  if (reason === "left") {
    const active = main.root.activeElement;
    if (host === null || !host.container.contains(active)) {
      closeDiagramIntent(main, "user", "none");
      // The focus went from the menu, not a stop, so nothing else takes the look off.
      if (host !== null) {
        clearStopLook(host);
      }
    }
  } else if (reason === "outside") {
    dismissingClick(main);
  }
}

/** Where a menu opened by a key stands, in the overlay's coordinates. */
function menuPlace(
  host: DiagramHost,
  target: TargetKey,
): { x: number; y: number } {
  const el = targetElement(host, target);
  const frame = host.overlay.getBoundingClientRect();
  if (el === null) {
    return { x: CANVAS_INSET, y: CANVAS_INSET };
  }
  const box = el.getBoundingClientRect();
  return target.kind === "edge" || target.kind === "message"
    ? {
        x: box.left - frame.left + box.width / 2,
        y: box.top - frame.top + box.height / 2,
      }
    : { x: box.left - frame.left, y: box.bottom - frame.top };
}

function openFlowMenu(
  main: EditorView,
  anchor: number,
  target: TargetKey,
  host: DiagramHost,
  at?: { x: number; y: number },
): void {
  if (!host.writable()) {
    return;
  }
  menus.get(main)?.handle?.close();
  // Connect mode waiting on another fence: its end must not reach this menu's hold.
  const waiting = connects.get(main);
  if (waiting !== undefined) {
    endConnect(waiting);
  }
  const fence = fenceNow(main, anchor);
  const model = fence && modelNow(fence);
  if (fence === null || model === null) {
    sayAt(main, anchor, "stale");
    return;
  }
  if (target.kind !== "canvas" && labelOfTarget(model, target) === null) {
    sayAt(main, anchor, "gone");
    return;
  }
  const { name, items } = menuOf(main, fence.from, target, model);
  main.dispatch({
    effects: [
      setDiagramIntent.of({ anchor: fence.from, target, mode: "menu" }),
      keepFenceDrawn.of(fence.from),
    ],
  });
  const open: OpenMenu = {
    main,
    handle: null,
    anchor: fence.from,
    target,
    body: fence.body,
    gone: false,
    back: null,
  };
  menus.set(main, open);
  prepareStop(host, target);
  const frame = host.overlay.getBoundingClientRect();
  open.handle = openMenu(
    host.overlay,
    at === undefined
      ? menuPlace(host, target)
      : { x: at.x - frame.left, y: at.y - frame.top },
    name,
    items,
    () => open.back,
    (reason) => {
      menuClosed(open, reason);
    },
  );
}

/** A peer's edit or a redraw: is what the menu says, and acts on, still right? */
function reconcileMenu(main: EditorView, open: OpenMenu): void {
  const { handle } = open;
  const intent = main.state.field(diagramIntent, false);
  if (handle === null || intent === undefined || intent === null) {
    // Its close is on the way, through `onIntentClosed`.
    return;
  }
  if (
    !handle.connected() ||
    intent.mode !== "menu" ||
    intent.anchor !== open.anchor
  ) {
    handle.close();
    return;
  }
  if (intentStale(main.state)) {
    return;
  }
  const fence = fenceNow(main, open.anchor);
  const model = fence && modelNow(fence);
  if (fence === null || model === null || fence.body === open.body) {
    return;
  }
  open.body = fence.body;
  const held = resolveHeldTarget(
    main,
    open.target,
    holds.get(main) ?? null,
    model,
    fence.bodyFrom,
  );
  if (held === "stale") {
    // Not said until it matters: an item answers with a sentence.
    return;
  }
  if (
    held === "gone" ||
    (held.kind !== "canvas" && labelOfTarget(model, held) === null)
  ) {
    open.gone = true;
    handle.close();
    sayAt(main, open.anchor, "gone");
    return;
  }
  if (JSON.stringify(held) !== JSON.stringify(open.target)) {
    open.target = held;
    main.dispatch({
      effects: setDiagramIntent.of({ ...intent, target: held }),
    });
  }
  handle.refresh(menuOf(main, open.anchor, held, model).items);
}

interface Connecting {
  main: EditorView;
  source: string;
  anchor: number;
  /** What the mode asks for, by the kind of diagram. */
  prompt: string;
  /** The prompt and its Cancel button. */
  bar: HTMLElement;
  /** A choice is being verified. */
  running: boolean;
  /** Left, by any route, while a choice was being verified: it is not written. */
  cancelled: boolean;
}

const connects = new WeakMap<EditorView, Connecting>();

function placeBar(open: Connecting, host: DiagramHost): void {
  if (open.bar.parentElement !== host.overlay) {
    host.overlay.append(open.bar);
  }
  host.container.classList.add(CONNECTING_CLASS);
}

function makeBar(main: EditorView, text: string): HTMLElement {
  const bar = document.createElement("div");
  bar.className = CONNECT_CLASS;
  const prompt = document.createElement("span");
  prompt.setAttribute("role", "status");
  prompt.textContent = text;
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.tabIndex = -1;
  cancel.textContent = "Cancel";
  cancel.addEventListener("mousedown", (event) => {
    event.preventDefault();
  });
  cancel.addEventListener("click", () => {
    cancelConnect(main);
  });
  bar.append(prompt, cancel);
  return bar;
}

/**
 * Takes the mode away. The focus is the caller's: a person who went elsewhere stays
 * there.
 */
function endConnect(open: Connecting): void {
  const { main, bar } = open;
  open.cancelled = true;
  connects.delete(main);
  bar.closest(`.${CONNECTING_CLASS}`)?.classList.remove(CONNECTING_CLASS);
  bar.remove();
  if (main.dom.isConnected) {
    releaseFence(main, open.anchor);
  }
}

function startConnect(main: EditorView, anchor: number, id: string): void {
  const fence = fenceToAct(main, anchor);
  if (fence === null) {
    return;
  }
  const model = modelNow(fence);
  const stop = model && connectStopOf(model, id);
  if (model === null || stop === null) {
    sayAt(main, anchor, "gone");
    return;
  }
  const prompt = model.kind === "sequence" ? SEQ_CONNECT_PROMPT : PROMPT;
  const open: Connecting = {
    main,
    source: id,
    anchor: fence.from,
    prompt,
    bar: makeBar(main, prompt),
    running: false,
    cancelled: false,
  };
  connects.set(main, open);
  focusTarget(main, fence.from, stop, "connect");
  main.dispatch({
    effects: [
      keepFenceDrawn.of(fence.from),
      // A live region that arrives with its text is commonly not read (R92).
      EditorView.announce.of(prompt),
    ],
  });
  const host = hostOf(main, fence.from);
  if (host !== null) {
    placeBar(open, host);
  }
}

/** The stop to stand on when the mode ends: the box it began from, if it stands. */
function backFromConnect(open: Connecting, to: string): TargetKey {
  const fence = fenceNow(open.main, open.anchor);
  const model = fence && modelNow(fence);
  return (model && connectStopOf(model, to)) ?? CANVAS;
}

function cancelConnect(main: EditorView): void {
  const open = connects.get(main);
  if (open === undefined) {
    return;
  }
  const place = placeNow(main, open.anchor);
  const stop = backFromConnect(open, open.source);
  endConnect(open);
  focusTarget(main, place, stop);
}

async function chooseBox(main: EditorView, to: string): Promise<void> {
  const open = connects.get(main);
  // A second key or click while the first is checked is the same choice.
  if (open === undefined || open.running) {
    return;
  }
  const place = placeNow(main, open.anchor);
  const fence = fenceToAct(main, place);
  if (fence === null) {
    return;
  }
  const sequence = modelNow(fence)?.kind === "sequence";
  open.running = true;
  // Whoever left during the check meant no arrow. Each kind's own check runs.
  const verify = async (body: string, dark: boolean): Promise<Verdict> => {
    const verdict = sequence
      ? await seqKind.verify(body, dark)
      : await flowKind.verify(body, dark);
    return open.cancelled
      ? { ok: false, reason: "the person left connect mode" }
      : verdict;
  };
  const outcome: ApplyOutcome<TargetKey> = sequence
    ? await runDiagramOp(
        main,
        place,
        { ...seqKind, verify },
        { op: "addMessage", from: open.source, to },
      )
    : await runDiagramOp(
        main,
        place,
        { ...flowKind, verify },
        { op: "connect", from: open.source, to },
      );
  open.running = false;
  if (open.cancelled || connects.get(main) !== open) {
    return;
  }
  const refused = "refused" in outcome ? outcome.refused : null;
  const anchor = placeNow(main, open.anchor);
  if (refused === "stale") {
    // Still waiting, and the next try may do: the sentence says so.
    sayAt(main, anchor, "stale");
    return;
  }
  const stop = backFromConnect(open, refused === null ? to : open.source);
  endConnect(open);
  focusTarget(main, anchor, stop);
  if (refused !== null) {
    sayAt(main, anchor, refused);
  } else if (sequence && outcome.applied && outcome.created !== undefined) {
    // A new message is named at once: its text is the second undo step.
    openLabelEditor(main, anchor, outcome.created, {
      selectAll: true,
      added: true,
    });
  }
}

registerConnectHooks({
  choose: (main, id) => {
    void chooseBox(main, id);
  },
  cancel: cancelConnect,
  remind: (main) => {
    const prompt = connects.get(main)?.prompt ?? PROMPT;
    main.dispatch({ effects: EditorView.announce.of(prompt) });
  },
});

/** A peer's edit or a redraw: is what connect mode holds still there? */
function reconcileConnect(main: EditorView, open: Connecting): void {
  const intent = main.state.field(diagramIntent, false);
  if (intent === undefined || intent === null) {
    return;
  }
  if (intent.mode !== "connect" || intent.anchor !== open.anchor) {
    endConnect(open);
    return;
  }
  const host = hostOf(main, open.anchor);
  if (host !== null) {
    placeBar(open, host);
  }
  if (open.running || intentStale(main.state)) {
    return;
  }
  const fence = fenceNow(main, open.anchor);
  const model = fence && modelNow(fence);
  if (model === null) {
    return;
  }
  const source = connectStopOf(model, open.source);
  if (source === null) {
    endConnect(open);
    focusTarget(main, open.anchor, CANVAS);
    sayAt(main, open.anchor, "gone");
    return;
  }
  // The box it stood on went: back to the one it began from.
  const standing = connectIdOf(intent.target);
  if (
    !connectable(intent.target) ||
    standing === null ||
    connectStopOf(model, standing) === null
  ) {
    focusTarget(main, open.anchor, source, "connect");
  }
}

registerStopAction("menu", openFlowMenu);

/** A box back in the window, with no menu to stand in it: the stop takes over. */
registerIntentReopen("menu", (main, intent, host) => {
  menus.get(main)?.handle?.close();
  const now = main.state.field(diagramIntent, false);
  backOnStop(main, host, now?.target ?? intent.target);
});

registerIntentReopen("connect", (main, intent, host) => {
  const open = connects.get(main);
  if (open !== undefined) {
    placeBar(open, host);
  }
  backOnStop(main, host, intent.target);
});

/** Views whose close listener is in: it outlives the plugin, as Raw takes that. */
const hooked = new WeakSet<EditorView>();

function onClosed(main: EditorView, reason: IntentCloseReason): void {
  const gone = reason === "gone";
  // A destroyed view's state still shows the intent, with nobody left to end it.
  const intent =
    reason === "destroy" ? null : main.state.field(diagramIntent, false);
  const menu = menus.get(main);
  const connect = connects.get(main);
  let told = false;
  // The close of an intent on another fence comes after a new one opened.
  if (
    menu !== undefined &&
    !(intent?.mode === "menu" && intent.anchor === menu.anchor)
  ) {
    menu.handle?.close();
    told = true;
  }
  if (
    connect !== undefined &&
    !(intent?.mode === "connect" && intent.anchor === connect.anchor)
  ) {
    endConnect(connect);
    told = true;
  }
  if (gone && told) {
    sayAt(main, menu?.anchor ?? connect?.anchor ?? 0, "gone");
  }
}

export const flowActions = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;

    constructor(view: EditorView) {
      this.view = view;
      if (!hooked.has(view)) {
        hooked.add(view);
        onIntentClosed(view, (reason) => {
          onClosed(view, reason);
        });
      }
    }

    update(update: ViewUpdate): void {
      const { view } = this;
      const menu = menus.get(view);
      const connect = connects.get(view);
      if (menu === undefined && connect === undefined) {
        return;
      }
      if (update.docChanged) {
        if (menu !== undefined) {
          menu.anchor = update.changes.mapPos(menu.anchor, 1);
        }
        if (connect !== undefined) {
          connect.anchor = update.changes.mapPos(connect.anchor, 1);
        }
      }
      queueMicrotask(() => {
        const nowMenu = menus.get(view);
        if (nowMenu !== undefined) {
          reconcileMenu(view, nowMenu);
        }
        const nowConnect = connects.get(view);
        if (nowConnect !== undefined) {
          reconcileConnect(view, nowConnect);
        }
      });
    }
  },
);
