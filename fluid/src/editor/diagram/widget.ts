/**
 * The drawn diagram: one box in the page holding a drawing layer, an overlay
 * layer (the rename field and the connect prompt live there) and a status line.
 *
 * What a box shows. The SVG on screen stays until a newer one has landed, and
 * a counter makes a late render of an older body lose, so an edit never blanks
 * the drawing. A failed mermaid import or a failed render is no verdict: what
 * is there stays, and a later render tries again.
 *
 * Where the height comes from (R54). A flip from the read-only preview takes
 * the SVG the preview already drew (`takePreviewDrawing`), so only the height
 * of the source lines moves. A fence coming back into the viewport pastes the
 * SVG it had, from a cache, in the same step. Anything else reserves a height
 * until its SVG arrives: the one measured earlier for this body, or an
 * estimate from the source. The same number is the widget's `estimatedHeight`,
 * which CodeMirror's height map uses while the fence is off screen.
 *
 * Which box is whose. `updateDOM` keeps a box through an edit of its fence.
 * CodeMirror also offers it any unclaimed box of this class, for example the
 * one of a fence that just scrolled away; that box holds another fence's
 * drawing and maybe its open rename field, so a box goes only with the fence
 * it was made for (`key`, kept by the field through changes). A line inserted
 * right at the first mark is the one edit CodeMirror offers nothing for and asks
 * for a new element: `toDOM` hands back the box that fence still has, as a
 * focused field inside it, and a composition, would not survive a new one.
 *
 * Going back to text. A preview has what a box showed in its first frame
 * (`offerPreviewDrawing`). With a refused body there is nothing to show, so
 * `flipBackHold` keeps the page still until the caption is there: one move.
 *
 * Render ids are unique across views for the life of the page: mermaid removes
 * any element that holds the id it is given, and works in a temporary `d<id>`
 * element in the body, which this module also removes when a render fails.
 *
 * Reading-only pages keep the drawing and its stops; `host.writable()` is what
 * the menus, the rename field, connect and delete ask before they offer
 * themselves. `focus.ts` makes the drawing's parts stops, and the box itself is
 * the canvas stop: with those, one place in the page's tab order.
 */

import { MapMode } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { EditorView as View, ViewPlugin, WidgetType } from "@codemirror/view";

import { initializeMermaid, mermaidThemeChanges } from "../../theme/mermaid";
import { writable } from "../elementEdit";
import {
  forgetPreviewOffers,
  holdPreviewHeight,
  offerPreviewDrawing,
  takePreviewDrawing,
} from "../fencePreviews";
import { diagramKindOf } from "./draws";
import type { RefusalCode } from "./edits";
import { NOTICES } from "./edits";
import { diagramField } from "./field";
import { RETRY_DELAYS_MS } from "./loader";

/** What the later layers (click mapping, rename, menus) reach a box through. */
export interface DiagramHost {
  container: HTMLElement;
  /** The drawing on screen, or null before the first one landed. */
  svg(): SVGSVGElement | null;
  /** The id of the drawing on screen; empty before the first one landed. */
  svgId(): string;
  overlay: HTMLElement;
  /** Says a refusal in the status line, for `NOTICE_MS`. */
  notice(code: RefusalCode): void;
  /** Keeps a sentence in the status line until the next one, or null to empty it. */
  holdNotice(text: string | null): void;
  /** The status line's id, for a field's `aria-describedby`. */
  statusId: string;
  /** The body of the SVG on screen; empty before the first one landed. */
  renderedBody(): string;
  /** Whether the page takes writes: menus, rename, connect and delete ask this. */
  writable(): boolean;
}

/** How long a notice stays. */
export const NOTICE_MS = 4_000;

/** Drawings kept for a fence coming back. */
const DRAWINGS_KEPT = 32;
/** Heights kept per body and theme. */
const HEIGHTS_KEPT = 256;
/** The tallest estimate: a long source is often wide rather than tall. */
const TALLEST_ESTIMATE = 800;

interface Box {
  view: EditorView;
  /** The fence this box belongs to (`diagramField`'s key). */
  key: number;
  dom: HTMLElement;
  drawing: HTMLElement;
  host: DiagramHost;
  status: HTMLElement;
  /** What the box should show. */
  body: string;
  dark: boolean;
  /** What it shows, as a draw key; null before the first drawing. */
  shown: string | null;
  shownBody: string;
  shownId: string;
  /** The draw key a render is running for, or null. */
  running: string | null;
  /** The newest render; an older one that lands is thrown away. */
  token: number;
  /** Failed tries of the current body. */
  failures: number;
  retry: ReturnType<typeof setTimeout> | null;
  /** The pace window a render opened (`RENDER_PACE_MS`). */
  pace: ReturnType<typeof setTimeout> | null;
  /** Whether a body came inside that window, so its end draws again. */
  owed: boolean;
  noticeTimer: ReturnType<typeof setTimeout> | null;
  dead: boolean;
  /** CodeMirror's elements for this box: a second one while it hands over. */
  claims: number;
  /** Its height at the last drawing; 0 before one was measured. */
  height: number;
}

const boxes = new WeakMap<HTMLElement, Box>();
/** Each view's boxes on screen, by their fence's key. */
const boxesByKey = new WeakMap<EditorView, Map<number, Box>>();

let renders = 0;
let statuses = 0;
let renderer: Promise<
  [typeof import("mermaid"), typeof import("../../theme/mermaidText")]
> | null = null;

/** Drawings by draw key, oldest first: the markup with the id it was drawn under. */
const drawings = new Map<string, { svg: string; id: string }>();
/** Ids on screen now: a drawing pasted while its id is here gets a new one. */
const liveIds = new Set<string>();
const heights = new Map<string, number>();

/** For tests: a body drawn by one test is not at hand in the next. */
export function forgetDrawings(): void {
  drawings.clear();
  heights.clear();
  forgetPreviewOffers();
}

function drawKey(body: string, dark: boolean): string {
  return `${dark ? "dark" : "light"}\n${body}`;
}

function keep<V>(map: Map<string, V>, key: string, value: V, limit: number) {
  map.delete(key);
  map.set(key, value);
  while (map.size > limit) {
    const oldest = map.keys().next().value;
    if (oldest === undefined) {
      break;
    }
    map.delete(oldest);
  }
}

/** A rough height from the source: one rank per statement, top to bottom. */
function estimateHeight(body: string): number {
  const statements = Math.max(
    1,
    body.split("\n").filter((line) => line.trim() !== "").length - 1,
  );
  if (diagramKindOf(body) === "sequenceDiagram") {
    // Measured on the starter: the boxes above and below, 46 px a statement.
    return Math.min(TALLEST_ESTIMATE, 120 + 46 * statements);
  }
  const sideways = /^\s*(?:flowchart|graph)\s+(?:LR|RL|>|<)/m.test(body);
  return Math.min(
    TALLEST_ESTIMATE,
    // Measured on a plain chain: 94 px a rank, top to bottom.
    sideways ? 96 + 40 * statements : 92 + 94 * statements,
  );
}

function heightFor(body: string, dark: boolean): number {
  return heights.get(drawKey(body, dark)) ?? estimateHeight(body);
}

type BoxListener = (host: DiagramHost) => void;

const mountListeners = new WeakMap<EditorView, Set<BoxListener>>();
const drawnListeners = new WeakMap<EditorView, Set<BoxListener>>();

function listen(
  registry: WeakMap<EditorView, Set<BoxListener>>,
  main: EditorView,
  listener: BoxListener,
): () => void {
  const set = registry.get(main) ?? new Set<BoxListener>();
  registry.set(main, set);
  set.add(listener);
  return () => {
    set.delete(listener);
  };
}

/** Told once for each new box, after the update that made it. */
export function onBoxMounted(
  main: EditorView,
  listener: BoxListener,
): () => void {
  return listen(mountListeners, main, listener);
}

/**
 * Told each time a box has a new drawing on screen, outside any update. Task
 * 10 decorates the SVG here, and an open mode moves to the new elements.
 */
export function onDiagramDrawn(
  main: EditorView,
  listener: BoxListener,
): () => void {
  return listen(drawnListeners, main, listener);
}

function tell(registry: WeakMap<EditorView, Set<BoxListener>>, box: Box): void {
  queueMicrotask(() => {
    if (box.dead) {
      return;
    }
    for (const listener of registry.get(box.view) ?? []) {
      listener(box.host);
    }
  });
}

/**
 * What scrolls the page when CodeMirror's own scroller does not, or null when
 * it does: its own scroller keeps the first line in the window through a height
 * change, and one further out knows nothing about the editor's lines.
 */
function outerScroller(view: EditorView): Element | null {
  const own = view.scrollDOM;
  if (own.scrollHeight > own.clientHeight + 1) {
    return null;
  }
  for (let el = own.parentElement; el !== null; el = el.parentElement) {
    if (
      /auto|scroll|overlay/.test(getComputedStyle(el).overflowY) &&
      el.scrollHeight > el.clientHeight + 1
    ) {
      return el;
    }
  }
  return document.scrollingElement;
}

function windowTopOf(scroller: Element): number {
  return scroller === document.scrollingElement
    ? 0
    : scroller.getBoundingClientRect().top;
}

/**
 * The kind of value CodeMirror's scroll requests carry (`scrollIntoView`, a
 * snapshot): their effect type is not exported, its value's class is.
 */
export const ScrollRequest = (View.scrollIntoView(0).value as object)
  .constructor as abstract new (...args: never[]) => object;

/**
 * How far under the window's top CodeMirror looks for the line its anchor step
 * keeps still (`scrollAnchorAt`): a box ending above that is wholly above to it.
 */
const ANCHOR_REACH_PX = 12;

/** Whether an update asks CodeMirror to scroll to a target. */
function asksScroll(update: ViewUpdate): boolean {
  return update.transactions.some(
    (tr) =>
      tr.scrollIntoView ||
      tr.effects.some((effect) => effect.value instanceof ScrollRequest),
  );
}

/** Views with a scroll to a target asked for since their last measure pass. */
const scrollAsks = new WeakSet<EditorView>();

/**
 * Notes an update that asks CodeMirror to scroll to a target (an undo, a key
 * that moves the caret, a scroll effect): it does so in the next measure pass.
 */
export const scrollAskWatcher = ViewPlugin.define((view) => ({
  update(update: ViewUpdate) {
    if (!asksScroll(update)) {
      return;
    }
    scrollAsks.add(view);
    view.requestMeasure({
      key: scrollAsks,
      read: () => null,
      write: () => {
        // That pass is over once the queue gets here.
        queueMicrotask(() => {
          scrollAsks.delete(view);
        });
      },
    });
  },
}));

/** The first of the editor's elements whose top is at or below `top`. */
function lineAtOrBelow(view: EditorView, top: number): Element | null {
  const children = view.contentDOM.children;
  let low = 0;
  let high = children.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    const child = children[middle];
    if (child !== undefined && child.getBoundingClientRect().top < top) {
      low = middle + 1;
    } else {
      high = middle;
    }
  }
  return children[low] ?? null;
}

/**
 * A fence first drawn wholly above the window, in an update that asks for no
 * scroll: its box is taller or shorter than its source lines were. CodeMirror's
 * anchor step keeps the text in the window still through that, but not in a
 * pass where its scroll offset moved since its last one (something above the
 * editor grew in between): then it keeps nothing. So the line at the window's
 * top is noted before the box replaces the source, and whatever of the change
 * the pass left goes back. A box first drawn in the window or under it moves
 * the text below it (R77).
 */
export const firstDrawHold = ViewPlugin.define((view) => ({
  update(update: ViewUpdate) {
    const before = update.startState.field(diagramField, false);
    const after = update.state.field(diagramField, false);
    if (before === undefined || after === undefined) {
      return;
    }
    if (before.fences === after.fences || asksScroll(update)) {
      return;
    }
    const known = new Set(before.keys.values());
    const fresh = [...after.keys.values()].filter((key) => !known.has(key));
    if (fresh.length === 0) {
      return;
    }
    const scroller = outerScroller(view);
    if (scroller === null) {
      return;
    }
    // Before CodeMirror's DOM sync: the source lines are still on the page.
    const line = lineAtOrBelow(view, windowTopOf(scroller));
    if (line === null) {
      return;
    }
    // Where the line is in the document: CodeMirror's sync below still has the
    // old one, the changes take it to the new.
    const at = update.changes.mapPos(view.posAtDOM(line, 0));
    const offset =
      line.getBoundingClientRect().top - view.dom.getBoundingClientRect().top;
    view.requestMeasure({
      read: () => {
        const windowTop = windowTopOf(scroller);
        const above = fresh.some((key) => {
          const box = boxesByKey.get(view)?.get(key);
          return (
            box !== undefined &&
            !box.dead &&
            box.dom.getBoundingClientRect().bottom <=
              windowTop + ANCHOR_REACH_PX
          );
        });
        // A box shorter than its source at the document's end: the browser
        // clamped the scroll in this layout, and that is in the editor's top.
        const atEnd =
          scroller.scrollTop >=
          scroller.scrollHeight - scroller.clientHeight - 1;
        // The editor's top before any scroll of the pass: the person's is in it.
        return {
          above,
          atEnd,
          editorTop: view.dom.getBoundingClientRect().top,
        };
      },
      write: ({ above, atEnd, editorTop }) => {
        if (!above || scrollAsks.has(view)) {
          return;
        }
        // After the pass, and after a landing's own step in it: `settleScroll`
        // is queued in a write of the same pass, so it is ahead of the first
        // hop, and the second hop runs after it. The two depend on each other:
        // run this first and `settleScroll` reads its scroll as CodeMirror's.
        queueMicrotask(() => {
          queueMicrotask(() => {
            // CodeMirror reuses a line's element, also for another line.
            if (
              !view.dom.isConnected ||
              !line.isConnected ||
              view.posAtDOM(line, 0) !== at
            ) {
              return;
            }
            const off = line.getBoundingClientRect().top - (editorTop + offset);
            // CodeMirror leaves a pixel or less as it is; the clamp kept a shrink.
            if (Math.abs(off) >= 1 && !(off < 0 && atEnd)) {
              scroller.scrollBy({ top: off, behavior: "instant" });
            }
          });
        });
      },
    });
  },
}));

/** What the boxes that landed above the window in one measure pass grew by. */
interface Settle {
  scroller: Element;
  /** The editor's top in the pass's reads, before any scroll of the pass. */
  editorTop: number;
  /** Boxes wholly above the window. */
  above: number;
  /** Boxes across the window's top. */
  across: number;
}

const settles = new WeakMap<EditorView, Settle>();

/**
 * After CodeMirror's pass, before the frame is painted. In a pass with no scroll
 * to a target, a scroll of CodeMirror's is its anchor step: it kept the line at
 * the window's top still through every change above it, a box wholly above
 * among them. It keeps a box across the top still itself, so the text under
 * that box is this step's in any case. `firstDrawHold` waits two microtasks so
 * that it runs after this one: keep this a single hop from the pass's write.
 */
function settleScroll(view: EditorView, settle: Settle): void {
  settles.delete(view);
  if (!view.dom.isConnected) {
    return;
  }
  const scrolled = settle.editorTop - view.dom.getBoundingClientRect().top;
  // CodeMirror leaves a pixel or less as it is.
  const off = settle.across + (Math.abs(scrolled) < 1 ? settle.above : 0);
  if (off !== 0) {
    settle.scroller.scrollBy({ top: off, behavior: "instant" });
  }
}

/**
 * The drawing is on screen: its id counts, its height is read, the map hears.
 * `before` is the box's height a moment ago, when it was on screen already: a
 * box that starts above the window then grew or shrank above what the person is
 * reading, so the text in the window is put back before the frame is painted.
 */
function landed(
  box: Box,
  key: string,
  body: string,
  id: string,
  before: number | null,
): void {
  liveIds.delete(box.shownId);
  liveIds.add(id);
  box.shown = key;
  box.shownBody = body;
  box.shownId = id;
  box.dom.style.minHeight = "";
  box.failures = 0;
  const { view } = box;
  view.requestMeasure({
    read: () => {
      const rect = box.dom.getBoundingClientRect();
      const scroller = before === null ? null : outerScroller(view);
      if (scroller === null || before === null) {
        return { height: rect.height, scroller: null, across: false, top: 0 };
      }
      const windowTop = windowTopOf(scroller);
      // A box that shrank with the page at its last position: the browser's
      // clamp of the scroll has already kept the text still.
      const clamped =
        rect.height < before &&
        scroller.scrollTop >= scroller.scrollHeight - scroller.clientHeight - 1;
      return {
        height: rect.height,
        scroller: rect.top < windowTop && !clamped ? scroller : null,
        // Its bottom as CodeMirror's height map still has it.
        across: rect.top + before > windowTop + ANCHOR_REACH_PX,
        top: view.dom.getBoundingClientRect().top,
      };
    },
    write: ({ height, scroller, across, top }) => {
      if (height > 0) {
        keep(heights, key, height, HEIGHTS_KEPT);
        box.height = height;
      }
      if (scroller === null || before === null || height === before) {
        return;
      }
      const growth = height - before;
      if (scrollAsks.has(view)) {
        // CodeMirror scrolls to the target after these writes, from the new
        // layout, and keeps no line still in that pass: the growth goes now.
        scroller.scrollBy({ top: growth, behavior: "instant" });
        return;
      }
      let settle = settles.get(view);
      if (settle === undefined) {
        const fresh = { scroller, editorTop: top, above: 0, across: 0 };
        settles.set(view, fresh);
        // Runs once CodeMirror's measure pass is over, still before the paint.
        queueMicrotask(() => {
          settleScroll(view, fresh);
        });
        settle = fresh;
      }
      if (across) {
        settle.across += growth;
      } else {
        settle.above += growth;
      }
    },
  });
  tell(drawnListeners, box);
}

function paste(box: Box, key: string, body: string, svg: string, id: string) {
  // On a drawing from the cache this runs inside CodeMirror's update, and the
  // read lays the page out there: the height before the drawing is known
  // nowhere else.
  const before = box.dom.isConnected
    ? box.dom.getBoundingClientRect().height
    : null;
  box.drawing.innerHTML = svg;
  offerPreviewDrawing(body, box.dark, svg, id);
  landed(box, key, body, id, before);
}

function nextRenderId(): string {
  renders += 1;
  return `cm-diagram-${String(renders)}`;
}

/** One load for all boxes; a failed one is forgotten, so a later render tries again. */
function loadRenderer() {
  renderer ??= Promise.all([
    import("mermaid"),
    import("../../theme/mermaidText"),
  ]).catch((cause: unknown) => {
    renderer = null;
    throw cause;
  });
  return renderer;
}

/** Starts a render of what the box should show. The old SVG stays meanwhile. */
function render(box: Box): void {
  const { body, dark } = box;
  const key = drawKey(body, dark);
  box.token += 1;
  const token = box.token;
  box.running = key;
  const id = nextRenderId();
  void loadRenderer()
    .then(async ([{ default: mermaid }, { drawnLabels }]) => {
      // Superseded before its turn: mermaid would draw a body nobody shows.
      if (token !== box.token || box.dead) {
        return null;
      }
      const theme = initializeMermaid(mermaid, dark);
      const rendered = await mermaid.render(id, body);
      return {
        svg: drawnLabels(rendered.svg),
        sameTheme: theme === mermaidThemeChanges(),
      };
    })
    .then(
      (drawn) => {
        if (drawn === null) {
          return;
        }
        const { svg, sameTheme } = drawn;
        // Mermaid queues its renders and reads the configuration when a render's
        // turn comes, so one that waited across a theme change drew in the other
        // theme. It is no drawing of this key, for this box or for the cache.
        if (sameTheme) {
          // Worth keeping whoever asked: an undo brings this body back.
          keep(drawings, key, { svg, id }, DRAWINGS_KEPT);
        }
        if (token === box.token && !box.dead) {
          box.running = null;
          if (sameTheme) {
            paste(box, key, body, svg, id);
          } else {
            render(box);
          }
        }
      },
      () => {
        if (token === box.token && !box.dead) {
          box.running = null;
          tryAgainLater(box);
        }
      },
    )
    .finally(() => {
      // Mermaid leaves this behind when it stops between its own clean-ups.
      document.getElementById(`d${id}`)?.remove();
    });
}

/** No verdict: what is on screen stays, and the waits are the loader's. */
function tryAgainLater(box: Box): void {
  const wait = RETRY_DELAYS_MS[box.failures];
  box.failures += 1;
  if (wait === undefined) {
    // Given up for now: the next edit, or the fence coming back, tries again.
    return;
  }
  box.retry = setTimeout(() => {
    box.retry = null;
    render(box);
  }, wait);
}

function cancelRetry(box: Box): void {
  if (box.retry !== null) {
    clearTimeout(box.retry);
    box.retry = null;
  }
}

/** The box's name for a screen reader, by the kind its body says. */
function boxName(body: string): string {
  return diagramKindOf(body) === "sequenceDiagram"
    ? "Sequence diagram"
    : "Flowchart";
}

/** Brings the box to what it should show, at once when a drawing is at hand. */
function draw(box: Box): void {
  box.dom.setAttribute("aria-label", boxName(box.body));
  const key = drawKey(box.body, box.dark);
  cancelRetry(box);
  if (box.shown === key) {
    // Back to what is on screen: a render still running is out of date.
    box.token += 1;
    box.running = null;
    box.owed = false;
    return;
  }
  if (box.running === key) {
    box.owed = false;
    return;
  }
  const cached = drawings.get(key);
  if (cached !== undefined) {
    box.token += 1;
    box.running = null;
    box.owed = false;
    keep(drawings, key, cached, DRAWINGS_KEPT);
    if (liveIds.has(cached.id)) {
      // The same body in another box on screen: this copy gets an id of its own.
      const id = nextRenderId();
      paste(box, key, box.body, cached.svg.split(cached.id).join(id), id);
    } else {
      paste(box, key, box.body, cached.svg, cached.id);
    }
    return;
  }
  box.failures = 0;
  pacedRender(box);
}

/**
 * How long a render waits for the next body before it starts (F5). A box with
 * nothing on screen yet, and the first new body after a quiet spell, render at
 * once; the bodies of a burst (a peer typing, verdicts landing one after the
 * other) render once, this long after the last of them, with the drawing on
 * screen staying meanwhile. Mermaid queues renders, so without it every
 * intermediate body would be drawn in turn. The same 200 ms as the verdict's
 * pace (`VERDICT_PACE_MS`), and for the same reason.
 */
export const RENDER_PACE_MS = 200;

/** `render`, paced by `RENDER_PACE_MS` once the box shows a drawing. */
function pacedRender(box: Box): void {
  if (box.shown === null) {
    render(box);
    return;
  }
  if (box.pace !== null) {
    clearTimeout(box.pace);
    box.owed = true;
  } else {
    render(box);
  }
  box.pace = setTimeout(() => {
    box.pace = null;
    if (box.owed && !box.dead) {
      box.owed = false;
      // What the box should show now, not what it was asked to show then.
      draw(box);
    }
  }, RENDER_PACE_MS);
}

function cancelPace(box: Box): void {
  if (box.pace !== null) {
    clearTimeout(box.pace);
    box.pace = null;
  }
  box.owed = false;
}

function makeBox(view: EditorView, widget: DiagramWidget): Box {
  const dom = document.createElement("div");
  dom.className = "cm-diagram";
  dom.setAttribute("role", "group");
  dom.setAttribute("aria-roledescription", "diagram");
  dom.setAttribute("aria-label", boxName(widget.body));
  dom.tabIndex = 0;
  const drawing = document.createElement("div");
  drawing.className = "cm-diagram-drawing";
  const overlay = document.createElement("div");
  overlay.className = "cm-diagram-overlay";
  const status = document.createElement("div");
  status.className = "cm-diagram-notice";
  statuses += 1;
  status.id = `cm-notice-${String(statuses)}`;
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  dom.append(drawing, overlay, status);
  const box: Box = {
    view,
    key: widget.key,
    dom,
    drawing,
    status,
    body: widget.body,
    dark: widget.dark,
    shown: null,
    shownBody: "",
    shownId: "",
    running: null,
    token: 0,
    failures: 0,
    retry: null,
    pace: null,
    owed: false,
    noticeTimer: null,
    dead: false,
    claims: 1,
    height: 0,
    host: {
      container: dom,
      overlay,
      svg: () => drawing.querySelector<SVGSVGElement>(":scope > svg"),
      svgId: () => drawing.querySelector(":scope > svg")?.id ?? "",
      notice: (code) => {
        notice(box, code);
      },
      holdNotice: (text) => {
        if (box.noticeTimer !== null) {
          clearTimeout(box.noticeTimer);
          box.noticeTimer = null;
        }
        status.textContent = text ?? "";
      },
      statusId: status.id,
      renderedBody: () => box.shownBody,
      writable: () => writable(view.state),
    },
  };
  boxes.set(dom, box);
  const byKey = boxesByKey.get(view) ?? new Map<number, Box>();
  boxesByKey.set(view, byKey);
  byKey.set(widget.key, box);
  return box;
}

function notice(box: Box, code: RefusalCode): void {
  if (box.noticeTimer !== null) {
    clearTimeout(box.noticeTimer);
  }
  box.status.textContent = NOTICES[code];
  box.noticeTimer = setTimeout(() => {
    box.noticeTimer = null;
    box.status.textContent = "";
  }, NOTICE_MS);
}

export class DiagramWidget extends WidgetType {
  readonly body: string;
  readonly dark: boolean;
  /** The fence this widget draws, as the field tracks it through changes. */
  readonly key: number;
  /** The document line the body starts on: which preview is this fence's. */
  readonly firstLine: number;

  constructor(body: string, dark: boolean, key: number, firstLine: number) {
    super();
    this.body = body;
    this.dark = dark;
    this.key = key;
    this.firstLine = firstLine;
  }

  override eq(other: DiagramWidget): boolean {
    // The key too: two fences of one body are still two boxes, each with its
    // own overlay, and CodeMirror asks this first when it hands out boxes.
    return (
      other.body === this.body &&
      other.dark === this.dark &&
      other.key === this.key
    );
  }

  /** Read when the height map needs it, so a measured height counts at once. */
  override get estimatedHeight(): number {
    return heightFor(this.body, this.dark);
  }

  override toDOM(view: EditorView): HTMLElement {
    // Rests on CodeMirror's sync moving no node that stays in order. An upgrade that
    // re-inserts one cuts a field's focus and composition; `widget.test.ts`'s "keeps
    // the box element" and the composition journey in `diagram-rename.spec.ts` go red.
    const standing = boxesByKey.get(view)?.get(this.key);
    if (standing !== undefined && !standing.dead) {
      standing.claims += 1;
      standing.body = this.body;
      standing.dark = this.dark;
      draw(standing);
      return standing.dom;
    }
    const box = makeBox(view, this);
    const key = drawKey(this.body, this.dark);
    const taken = takePreviewDrawing(
      view.contentDOM,
      this.body,
      this.dark,
      this.firstLine,
    );
    if (taken === null) {
      box.dom.style.minHeight = `${String(heightFor(this.body, this.dark))}px`;
      draw(box);
    } else {
      box.drawing.append(taken);
      keep(
        drawings,
        key,
        { svg: taken.outerHTML, id: taken.id },
        DRAWINGS_KEPT,
      );
      offerPreviewDrawing(this.body, this.dark, taken.outerHTML, taken.id);
      landed(box, key, this.body, taken.id, null);
    }
    tell(mountListeners, box);
    return box.dom;
  }

  override updateDOM(dom: HTMLElement): boolean {
    const box = boxes.get(dom);
    if (box === undefined || box.key !== this.key) {
      return false;
    }
    box.body = this.body;
    box.dark = this.dark;
    draw(box);
    return true;
  }

  override destroy(dom: HTMLElement): void {
    const box = boxes.get(dom);
    if (box === undefined) {
      return;
    }
    box.claims -= 1;
    if (box.claims > 0) {
      return;
    }
    box.dead = true;
    const byKey = boxesByKey.get(box.view);
    if (byKey?.get(box.key) === box) {
      byKey.delete(box.key);
    }
    cancelRetry(box);
    cancelPace(box);
    if (box.noticeTimer !== null) {
      clearTimeout(box.noticeTimer);
    }
    liveIds.delete(box.shownId);
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

/**
 * The box of the drawn fence that holds `anchor`, or null when no fence is
 * drawn there or it is off screen. Not during an update: it reads positions.
 */
export function hostOf(main: EditorView, anchor: number): DiagramHost | null {
  const state = main.state;
  const fence = state
    .field(diagramField, false)
    ?.fences.find((f) => f.from <= anchor && anchor <= f.to);
  if (fence === undefined) {
    return null;
  }
  const start = state.doc.lineAt(fence.from).from;
  for (const dom of main.contentDOM.querySelectorAll<HTMLElement>(
    ".cm-diagram",
  )) {
    const box = boxes.get(dom);
    if (box !== undefined && !box.dead && main.posAtDOM(dom) === start) {
      return box.host;
    }
  }
  return null;
}

/** The room a caption usually takes, so one landing moves the text no further. */
const CAPTION_ROOM = 48;

/**
 * A fence that stops being drawn (a refused body, or the source shown): its
 * preview holds the height the box had less the source lines, until its answer is
 * there, so the page moves once and not twice.
 */
export const flipBackHold = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;

    constructor(view: EditorView) {
      this.view = view;
    }

    update(update: ViewUpdate): void {
      const before = update.startState.field(diagramField, false)?.fences;
      const after = update.state.field(diagramField, false)?.fences;
      if (before === undefined || after === undefined || before === after) {
        return;
      }
      for (const fence of before) {
        const at = update.changes.mapPos(fence.from, 1, MapMode.TrackAfter);
        if (at !== null && !after.some((f) => f.from === at)) {
          // The box as it stood: a carried fence's body is one never drawn.
          const key = update.startState
            .field(diagramField)
            .keys.get(fence.from);
          const height =
            key === undefined
              ? 0
              : (boxesByKey.get(this.view)?.get(key)?.height ?? 0);
          if (height > 0) {
            this.hold(at, height);
          }
        }
      }
    }

    hold(from: number, height: number): void {
      const { view } = this;
      // After the update, whose DOM this reads, and before any frame.
      queueMicrotask(() => {
        const { doc } = view.state;
        if (from > doc.length) {
          return;
        }
        const line = doc.lineAt(from);
        // The line's own top: a character's stands a little below it.
        const { node } = view.domAtPos(line.from);
        const top = (node instanceof Element ? node : node.parentElement)
          ?.closest(".cm-line")
          ?.getBoundingClientRect().top;
        const previewBox = [
          ...view.contentDOM.querySelectorAll<HTMLElement>(
            ".cm-mermaid-preview",
          ),
        ].find((el) => doc.lineAt(view.posAtDOM(el)).number > line.number);
        if (top !== undefined && previewBox !== undefined) {
          const source = previewBox.getBoundingClientRect().top - top;
          holdPreviewHeight(
            previewBox,
            Math.max(height - source, CAPTION_ROOM),
          );
        }
      });
    }
  },
);

export const diagramTheme = View.baseTheme({
  // The preview's padding and no more, so a drawing keeps its size in the flip.
  ".cm-diagram": {
    position: "relative",
    boxSizing: "border-box",
    minHeight: "3rem",
    padding: "0.5rem 0.75rem",
  },
  ".cm-diagram-overlay": {
    position: "absolute",
    inset: "0",
    pointerEvents: "none",
  },
  ".cm-diagram-overlay > *": { pointerEvents: "auto" },
  // The ring is the look of the focus; the browser's own outline would double it.
  ".cm-diagram:focus, .cm-diagram svg [tabindex]:focus": { outline: "none" },
  ".cm-diagram-refusal": {
    padding: "0.25rem 0.75rem",
    color: "var(--color-slate-700)",
    backgroundColor: "var(--color-slate-100)",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
  },
  "&dark .cm-diagram-refusal": {
    color: "var(--color-slate-200)",
    backgroundColor: "var(--color-slate-800)",
  },
  ".cm-diagram-overlay > .cm-diagram-focus-ring": {
    position: "absolute",
    pointerEvents: "none",
    boxSizing: "border-box",
    border: "2px solid var(--color-accent-600)",
    borderRadius: "0.25rem",
  },
  "&dark .cm-diagram-overlay > .cm-diagram-focus-ring": {
    borderColor: "var(--color-accent-300)",
  },
  // Over its box or arrow, and over the text below when a long label needs that.
  ".cm-diagram-overlay > .cm-diagram-label-field": {
    position: "absolute",
    zIndex: "1",
    boxSizing: "border-box",
    margin: "0",
    padding: "0.125rem 0.25rem",
    overflow: "hidden",
    resize: "none",
    textAlign: "center",
    font: "inherit",
    lineHeight: "1.25",
    color: "var(--color-slate-900)",
    backgroundColor: "var(--color-white, #ffffff)",
    border: "2px solid var(--color-accent-600)",
    borderRadius: "0.25rem",
    outline: "none",
  },
  "&dark .cm-diagram-overlay > .cm-diagram-label-field": {
    color: "var(--color-slate-100)",
    backgroundColor: "var(--color-slate-900)",
    borderColor: "var(--color-accent-300)",
  },
  ".cm-diagram-overlay > .cm-diagram-menu": {
    position: "absolute",
    zIndex: "2",
    boxSizing: "border-box",
    minWidth: "11rem",
    maxWidth: "20rem",
    overflowY: "auto",
    padding: "0.25rem",
    color: "var(--color-slate-900)",
    backgroundColor: "var(--color-white, #ffffff)",
    border: "1px solid var(--color-slate-300)",
    borderRadius: "0.375rem",
    boxShadow: "0 8px 20px rgb(0 0 0 / 0.18)",
    fontSize: "var(--text-body-sm, 0.875rem)",
    lineHeight: "1.25rem",
    fontFamily: "var(--font-sans, inherit)",
  },
  "&dark .cm-diagram-overlay > .cm-diagram-menu": {
    color: "var(--color-slate-100)",
    backgroundColor: "var(--color-slate-900)",
    borderColor: "var(--color-slate-700)",
  },
  ".cm-diagram-menu-item": {
    position: "relative",
    padding: "0.25rem 1.5rem",
    borderRadius: "0.25rem",
    cursor: "default",
    outline: "none",
    whiteSpace: "nowrap",
  },
  ".cm-diagram-menu-item:focus": {
    color: "var(--color-white, #ffffff)",
    backgroundColor: "var(--color-accent-600)",
  },
  ".cm-diagram-menu-item[aria-disabled]": { opacity: "0.55" },
  '.cm-diagram-menu-item[aria-checked="true"]::before': {
    content: '"\\2713"',
    position: "absolute",
    left: "0.375rem",
  },
  ".cm-diagram-menu-item[aria-haspopup]::after": {
    content: '"\\203A"',
    position: "absolute",
    right: "0.5rem",
  },
  // A disabled item's reason, for the eye: empty, it takes no room.
  ".cm-diagram-menu-hint": {
    whiteSpace: "normal",
    color: "var(--color-slate-600)",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
  },
  ".cm-diagram-menu-hint:not(:empty)": { padding: "0.25rem 0.5rem" },
  "&dark .cm-diagram-menu-hint": { color: "var(--color-slate-300)" },
  // In the window while any of the box is, and not in the way of a click below.
  ".cm-diagram-overlay > .cm-diagram-connect": {
    position: "sticky",
    top: "0.25rem",
    zIndex: "1",
    display: "flex",
    alignItems: "baseline",
    gap: "0.75rem",
    width: "fit-content",
    margin: "0 auto",
    padding: "0.125rem 0.5rem",
    pointerEvents: "none",
    color: "var(--color-slate-900)",
    backgroundColor: "var(--color-slate-100)",
    borderRadius: "0.25rem",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
  },
  "&dark .cm-diagram-overlay > .cm-diagram-connect": {
    color: "var(--color-slate-100)",
    backgroundColor: "var(--color-slate-800)",
  },
  ".cm-diagram-connect button": {
    pointerEvents: "auto",
    textDecoration: "underline",
    cursor: "pointer",
    color: "inherit",
    background: "none",
    border: "none",
    padding: "0",
    font: "inherit",
  },
  ".cm-diagram-connecting svg g.node": { cursor: "crosshair" },
  // Over the drawing, so a notice never moves the text below the box.
  ".cm-diagram-notice": {
    position: "absolute",
    left: "0.75rem",
    bottom: "0.25rem",
    pointerEvents: "none",
    color: "var(--color-slate-700)",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
  },
  ".cm-diagram-notice:not(:empty)": {
    backgroundColor: "var(--color-slate-100)",
    borderRadius: "0.25rem",
    padding: "0.125rem 0.375rem",
  },
  "&dark .cm-diagram-notice": { color: "var(--color-slate-200)" },
  "&dark .cm-diagram-notice:not(:empty)": {
    backgroundColor: "var(--color-slate-800)",
  },
});
