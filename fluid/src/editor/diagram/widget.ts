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
 * it was made for (`key`, kept by the field through changes).
 *
 * Render ids are unique across views for the life of the page: mermaid removes
 * any element that holds the id it is given, and works in a temporary `d<id>`
 * element in the body, which this module also removes when a render fails.
 *
 * Reading-only pages keep the drawing and its stops; `host.writable()` is what
 * the menus, the rename field, connect and delete ask before they offer
 * themselves. The stops come with Task 10, and the box itself is the canvas
 * stop from the start: the one place in the page's tab order.
 */

import type { EditorView } from "@codemirror/view";
import { EditorView as View, WidgetType } from "@codemirror/view";

import { mermaidConfig } from "../../theme/mermaid";
import { writable } from "../elementEdit";
import { takePreviewDrawing } from "../fencePreviews";
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
  noticeTimer: ReturnType<typeof setTimeout> | null;
  dead: boolean;
}

const boxes = new WeakMap<HTMLElement, Box>();

let renders = 0;
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

/**
 * The drawing is on screen: its id counts, its height is read, the map hears.
 * `before` is the box's height a moment ago, when it was on screen already: a
 * box that starts above the window then grew or shrank above what the person is
 * reading, so the scroller takes the difference in the same frame.
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
  box.view.requestMeasure({
    read: () => {
      const rect = box.dom.getBoundingClientRect();
      const scroller = before === null ? null : outerScroller(box.view);
      const windowTop =
        scroller === null || scroller === document.scrollingElement
          ? 0
          : scroller.getBoundingClientRect().top;
      return {
        height: rect.height,
        scroller: rect.top < windowTop ? scroller : null,
      };
    },
    write: ({ height, scroller }) => {
      if (height > 0) {
        keep(heights, key, height, HEIGHTS_KEPT);
      }
      if (scroller !== null && before !== null && height !== before) {
        scroller.scrollBy({ top: height - before, behavior: "instant" });
      }
    },
  });
  tell(drawnListeners, box);
}

function paste(box: Box, key: string, body: string, svg: string, id: string) {
  const before = box.dom.isConnected
    ? box.dom.getBoundingClientRect().height
    : null;
  box.drawing.innerHTML = svg;
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
      mermaid.initialize(mermaidConfig(dark));
      const rendered = await mermaid.render(id, body);
      return drawnLabels(rendered.svg);
    })
    .then(
      (svg) => {
        // Worth keeping whoever asked: an undo brings this body back.
        keep(drawings, key, { svg, id }, DRAWINGS_KEPT);
        if (token === box.token && !box.dead) {
          box.running = null;
          paste(box, key, body, svg, id);
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

/** Brings the box to what it should show, at once when a drawing is at hand. */
function draw(box: Box): void {
  const key = drawKey(box.body, box.dark);
  cancelRetry(box);
  if (box.shown === key) {
    // Back to what is on screen: a render still running is out of date.
    box.token += 1;
    box.running = null;
    return;
  }
  if (box.running === key) {
    return;
  }
  const cached = drawings.get(key);
  if (cached !== undefined) {
    box.token += 1;
    box.running = null;
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
  render(box);
}

function makeBox(view: EditorView, widget: DiagramWidget): Box {
  const dom = document.createElement("div");
  dom.className = "cm-diagram";
  dom.setAttribute("role", "group");
  dom.setAttribute("aria-roledescription", "diagram");
  dom.setAttribute("aria-label", "Flowchart");
  dom.tabIndex = 0;
  const drawing = document.createElement("div");
  drawing.className = "cm-diagram-drawing";
  const overlay = document.createElement("div");
  overlay.className = "cm-diagram-overlay";
  const status = document.createElement("div");
  status.className = "cm-diagram-notice";
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
    noticeTimer: null,
    dead: false,
    host: {
      container: dom,
      overlay,
      svg: () => drawing.querySelector<SVGSVGElement>(":scope > svg"),
      svgId: () => drawing.querySelector(":scope > svg")?.id ?? "",
      notice: (code) => {
        notice(box, code);
      },
      renderedBody: () => box.shownBody,
      writable: () => writable(view.state),
    },
  };
  boxes.set(dom, box);
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
    box.dead = true;
    cancelRetry(box);
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
