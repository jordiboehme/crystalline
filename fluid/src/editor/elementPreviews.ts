/**
 * The live preview of the dialect's elements: wherever a line hides its marks
 * (`marksHiddenOnLine`: every line by default, with the setting on every line
 * a selection does not touch), a callout, a status label, a colour code and
 * math draw the way the reading page draws them; elsewhere the text shows as
 * written. The rule `preview.ts` keeps for headings and emphasis, applied to
 * the elements it does not know.
 *
 * - A callout's lines wear the page's `.callout` rule and tint (its `> `
 *   marks fold away with every quote's, in the hidden-mark layer), and its
 *   head marker becomes the head the page draws:
 *   the fold chevron, the kind's icon, and the kind word when no title is
 *   written (a title stays as text, set in the head's weight).
 * - A status label becomes the page's pill.
 * - A colour code in inline code gets the page's swatch before it.
 * - Inline math (`` $`...`$ ``) and display math (a `math` fence, a `$$`
 *   block) are set by KaTeX through the reading page's own lazy `MathPart`,
 *   so KaTeX loads only once a document holds math.
 *
 * Nothing here writes the document: every decoration is a view over the
 * bytes. Positions come from the syntax tree, whose dialect nodes
 * (`dialectHighlight.ts`) are read as the reading page reads the source, and
 * the callout and label readers are `elementContext.ts`'s, so the bar and the
 * preview agree on where an element is.
 *
 * Two layers, because CodeMirror takes a block decoration only from a state
 * field: the inline elements are a view plugin over the visible ranges, the
 * display math is a field over the whole document.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import { StateField } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";
import { ChevronRight } from "lucide-react";
import type { MouseEvent as ReactMouseEvent, ReactElement } from "react";
import { createElement } from "react";
import type { Root } from "react-dom/client";
import { createRoot } from "react-dom/client";

import { CALLOUT_ICONS, CALLOUT_WORDS } from "../components/calloutFace";
import { lazyPart } from "../components/lazyPart";
import type { CalloutKind } from "../markdown/callouts";
import type { StatusColour } from "../markdown/directives";
import { labelPlainWords } from "../markdown/statusText";
import { SWATCH_CLASSES, colourOf } from "../markdown/swatch";
import {
  calloutAt,
  calloutOf,
  setCalloutFold,
  statusOf,
} from "./elementContext";
import type { ElementEdit } from "./elementEdit";
import {
  answersElementEdits,
  macMenuClick,
  pressElement,
  writable,
} from "./elementEdit";
import { calloutMarkAt, mathAt, statusAt } from "./elementVerbs";
import { frontmatterRegion } from "./frontmatterRegion";
import type { MathBlock } from "./mathBlocks";
import {
  escapedAt,
  mathBlocks,
  mathSections,
  openMathFences,
  unpadded,
} from "./mathBlocks";
import {
  hiddenMarkLayerOn,
  markModeChanged,
  marksHiddenOnLine,
} from "./marks/mode";
import { parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";

/** The reading page's formula part: its own chunk, KaTeX inside it. */
const MathPart = lazyPart(() => import("../components/MathPart"));

/** The React roots the widgets below draw into, by their element. */
const roots = new WeakMap<HTMLElement, Root>();

/**
 * Draw `element` into `dom`. Never synchronously: a widget can be drawn while
 * React is committing the screen that mounts the buffer, and the inline
 * pieces are one line tall whenever they land.
 */
function draw(dom: HTMLElement, element: ReactElement): void {
  const root = createRoot(dom);
  roots.set(dom, root);
  root.render(element);
}

/** Take a widget's React root down once the editor has let go of it. */
function undraw(dom: HTMLElement): void {
  const root = roots.get(dom);
  if (root !== undefined) {
    roots.delete(dom);
    // Never during a render React may be in the middle of.
    queueMicrotask(() => {
      root.unmount();
    });
  }
}

/**
 * A press on a drawn element asks for its popover (`pressElement`): only the
 * primary button, only where marks are hidden (never in Raw, never in a
 * table cell before it has the layer), only on a page that takes writes.
 */
function askOnPress(
  view: EditorView,
  dom: HTMLElement,
  event: MouseEvent,
  read: (pos: number) => ElementEdit | null,
): void {
  const { state } = view;
  if (
    event.button !== 0 ||
    macMenuClick(event) ||
    !state.facet(hiddenMarkLayerOn) ||
    !writable(state) ||
    !answersElementEdits(view)
  ) {
    return;
  }
  const pos = view.posAtDOM(dom);
  const edit = read(pos);
  if (edit !== null) {
    pressElement(view, event, edit, pos);
  }
}

/** A callout's head as the page draws it, in place of its marker. */
class CalloutHeadWidget extends WidgetType {
  readonly kind: CalloutKind;
  readonly fold: "open" | "closed" | null;
  readonly titled: boolean;

  constructor(
    kind: CalloutKind,
    fold: "open" | "closed" | null,
    titled: boolean,
  ) {
    super();
    this.kind = kind;
    this.fold = fold;
    this.titled = titled;
  }

  override eq(other: CalloutHeadWidget): boolean {
    return (
      other.kind === this.kind &&
      other.fold === this.fold &&
      other.titled === this.titled
    );
  }

  override toDOM(view: EditorView): HTMLElement {
    const dom = document.createElement("span");
    dom.className = "cm-callout-head";
    const Icon = CALLOUT_ICONS[this.kind];
    const icon = {
      "aria-hidden": true,
      size: 16,
      className: "shrink-0 text-(--callout)",
    };
    const kind = this.kind;
    const fold = this.fold;
    // The chevron flips the sign; a press must not also move the caret.
    const onFold = (event: ReactMouseEvent) => {
      if (event.button !== 0 || macMenuClick(event.nativeEvent)) return;
      event.preventDefault();
      const { state } = view;
      if (!state.facet(hiddenMarkLayerOn) || !writable(state)) return;
      setCalloutFold(
        view,
        fold === "open" ? "closed" : "open",
        view.posAtDOM(dom),
      );
    };
    const onKind = (event: ReactMouseEvent) => {
      askOnPress(view, dom, event.nativeEvent, (pos) => {
        const mark = calloutMarkAt(view.state, pos);
        return mark === null
          ? null
          : {
              kind: "callout-kind",
              at: mark.from,
              rect: dom.getBoundingClientRect(),
            };
      });
      // A press on the icon never places the caret in the marker.
      event.preventDefault();
    };
    const button = "inline-flex cursor-pointer items-center rounded-sm";
    draw(
      dom,
      createElement(
        "span",
        { className: "inline-flex items-center gap-1.5 align-[-0.15em]" },
        // The page's head: a chevron when it folds, the kind's icon unless
        // the kind is the fold itself, the word when nothing else names it.
        fold === null
          ? null
          : createElement(
              "button",
              {
                key: "fold",
                type: "button",
                tabIndex: -1,
                className: button,
                "aria-label": "Callout fold",
                "aria-expanded": fold === "open",
                onMouseDown: onFold,
              },
              createElement(ChevronRight, {
                ...icon,
                className: `${icon.className} ${fold === "open" ? "rotate-90" : ""}`,
              }),
            ),
        fold !== null && kind === "details"
          ? null
          : createElement(
              "button",
              {
                key: "kind",
                type: "button",
                tabIndex: -1,
                className: button,
                "aria-label": `Callout kind: ${CALLOUT_WORDS[kind]}`,
                "aria-haspopup": "menu",
                onMouseDown: onKind,
              },
              createElement(Icon, icon),
            ),
        this.titled
          ? null
          : createElement("span", { key: "word" }, CALLOUT_WORDS[kind]),
      ),
    );
    return dom;
  }

  override destroy(dom: HTMLElement): void {
    undraw(dom);
  }

  override ignoreEvent(event: Event): boolean {
    // A press on a button is the button's; any other click is a click into
    // the text: the caret lands on the head's line and the marker shows as
    // written.
    return (
      event.target instanceof Element && event.target.closest("button") !== null
    );
  }
}

/** A status label as the page's pill. */
class StatusWidget extends WidgetType {
  readonly label: string;
  readonly colour: StatusColour;

  constructor(label: string, colour: StatusColour) {
    super();
    this.label = label;
    this.colour = colour;
  }

  override eq(other: StatusWidget): boolean {
    return other.label === this.label && other.colour === this.colour;
  }

  override toDOM(view: EditorView): HTMLElement {
    const pill = document.createElement("span");
    pill.className = "status-label";
    pill.dataset["status"] = this.colour;
    pill.textContent = this.label;
    pill.addEventListener("mousedown", (event) => {
      askOnPress(view, pill, event, (pos) => {
        const status = statusAt(view.state, pos);
        return status === null
          ? null
          : {
              kind: "status",
              at: status.from,
              rect: pill.getBoundingClientRect(),
            };
      });
    });
    return pill;
  }

  override ignoreEvent(): boolean {
    return false;
  }
}

/** The page's swatch, before a colour code. */
class SwatchWidget extends WidgetType {
  readonly colour: string;

  constructor(colour: string) {
    super();
    this.colour = colour;
  }

  override eq(other: SwatchWidget): boolean {
    return other.colour === this.colour;
  }

  override toDOM(): HTMLElement {
    const swatch = document.createElement("span");
    swatch.dataset["swatch"] = "";
    swatch.setAttribute("aria-hidden", "true");
    swatch.className = SWATCH_CLASSES;
    swatch.style.background = this.colour;
    return swatch;
  }
}

/** The observers that re-measure a display formula, by its element. */
const observers = new WeakMap<HTMLElement, ResizeObserver>();

/** A formula, set by the reading page's lazy part. */
class MathWidget extends WidgetType {
  readonly tex: string;
  readonly display: boolean;
  /** The callout a display formula stands in, whose tint it keeps. */
  readonly callout: CalloutKind | null;
  /**
   * The callout's edge line classes (`cm-callout-first`, `cm-callout-last`)
   * of the lines the formula replaces, so the head and foot padding stay.
   */
  readonly edge: string;

  constructor(
    tex: string,
    display: boolean,
    callout: CalloutKind | null = null,
    edge = "",
  ) {
    super();
    this.tex = tex;
    this.display = display;
    this.callout = callout;
    this.edge = edge;
  }

  override eq(other: MathWidget): boolean {
    return (
      other.tex === this.tex &&
      other.display === this.display &&
      other.callout === this.callout &&
      other.edge === this.edge
    );
  }

  override toDOM(view: EditorView): HTMLElement {
    const dom: HTMLElement = document.createElement(
      this.display ? "div" : "span",
    );
    dom.className = this.display ? "cm-math-block" : "cm-math-inline";
    if (this.callout !== null) {
      dom.classList.add("callout");
      dom.dataset["callout"] = this.callout;
      for (const name of this.edge.split(" ").filter(Boolean)) {
        dom.classList.add(name);
      }
    }
    dom.addEventListener("mousedown", (event) => {
      askOnPress(view, dom, event, (pos) => {
        const math = mathAt(view.state, pos);
        return math === null
          ? null
          : {
              kind: "math",
              at: math.from,
              rect: dom.getBoundingClientRect(),
              block: math.block,
            };
      });
    });
    // The source as code until the part arrives, as on the page.
    const fallback = createElement("code", null, this.tex);
    draw(
      dom,
      createElement(MathPart, {
        fallback,
        tex: this.tex,
        display: this.display,
      }),
    );
    if (this.display) {
      // The formula is taller than its fallback, and KaTeX arrives when it
      // arrives: the editor measures the block again whenever it changes.
      const observer = new ResizeObserver(() => {
        view.requestMeasure();
      });
      observer.observe(dom);
      observers.set(dom, observer);
    }
    return dom;
  }

  override destroy(dom: HTMLElement): void {
    observers.get(dom)?.disconnect();
    observers.delete(dom);
    undraw(dom);
  }

  override ignoreEvent(): boolean {
    return false;
  }
}

/**
 * Whether line `number` shows its source rather than its elements: never with
 * marks hidden on it (`marksHiddenOnLine`), else while a selection touches it.
 */
function showsSource(state: EditorState, number: number): boolean {
  return !marksHiddenOnLine(state, number);
}

function calloutDecorations(
  state: EditorState,
  quote: SyntaxNode,
  out: Range<Decoration>[],
): void {
  const callout = calloutOf(state, quote);
  if (callout === null) {
    return;
  }
  const doc = state.doc;
  const first = doc.lineAt(quote.from).number;
  const last = doc.lineAt(quote.to).number;
  // A details callout folds without a sign, as on the page.
  const fold =
    callout.fold ?? (callout.callout === "details" ? "closed" : null);
  for (let number = first; number <= last; number += 1) {
    const line = doc.line(number);
    // The first and last lines carry the page's head and foot padding.
    const edge = [
      number === first ? "cm-callout-first" : "",
      number === last ? "cm-callout-last" : "",
    ].join(" ");
    // The `> ` marks are the hidden-mark layer's, as on any quote.
    out.push(
      Decoration.line({
        class: `callout ${edge}`.trim(),
        attributes: { "data-callout": callout.callout },
      }).range(line.from),
    );
  }
  if (showsSource(state, callout.line)) {
    return;
  }
  const head = doc.lineAt(callout.markFrom);
  const after = doc.sliceString(callout.markTo, head.to);
  const title = after.trim();
  // The marker and the space after it: a title then starts where the head's
  // words start on the page.
  const space = /^[ \t]*/.exec(after)?.[0].length ?? 0;
  out.push(
    Decoration.replace({
      widget: new CalloutHeadWidget(callout.callout, fold, title !== ""),
    }).range(callout.markFrom, callout.markTo + space),
  );
  if (title !== "") {
    out.push(
      Decoration.mark({ class: "cm-callout-title" }).range(
        callout.markTo + space,
        head.to,
      ),
    );
  }
}

/** The text inside an inline code span, between its backtick marks. */
function codeInside(node: SyntaxNode): [number, number] {
  const open = node.firstChild;
  const close = node.lastChild;
  if (open === null || close === null || open === close) {
    return [node.from, node.to];
  }
  return [open.to, close.from];
}

/** The decorations, and the hidden spans a caret steps over. */
interface Inline {
  decorations: DecorationSet;
  hidden: DecorationSet;
}

function buildInline(view: EditorView): Inline {
  const { state } = view;
  const doc = state.doc;
  const fmEnd = frontmatterRegion(doc)?.to ?? -1;
  const out: Range<Decoration>[] = [];
  // A node two visible ranges both reach is decorated once.
  const seen = new Set<string>();
  const once = (node: SyntaxNode) => {
    const key = `${node.name}:${node.from}`;
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
    return true;
  };
  for (const { from, to } of view.visibleRanges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter: (ref) => {
        if (ref.from <= fmEnd) {
          return;
        }
        if (ref.name === "FencedCode" || ref.name === "CodeBlock") {
          return false;
        }
        const node = ref.node;
        if (ref.name === "Blockquote") {
          if (once(node)) {
            calloutDecorations(state, node, out);
          }
          return;
        }
        const lineActive = showsSource(state, doc.lineAt(ref.from).number);
        if (ref.name === "StatusLabel") {
          const status = statusOf(state, node);
          if (status !== null && !lineActive && once(node)) {
            const open = node.firstChild;
            const label = doc.sliceString(
              open?.to ?? node.from,
              status.closeFrom,
            );
            out.push(
              Decoration.replace({
                widget: new StatusWidget(labelPlainWords(label), status.colour),
              }).range(status.from, status.to),
            );
          }
          return false;
        }
        if (ref.name === "InlineMath") {
          const open = node.firstChild;
          const close = node.lastChild;
          if (!lineActive && open !== null && close !== null && once(node)) {
            out.push(
              Decoration.replace({
                widget: new MathWidget(
                  unpadded(doc.sliceString(open.to, close.from)),
                  false,
                ),
              }).range(ref.from, ref.to),
            );
          }
          return false;
        }
        if (ref.name === "InlineCode") {
          if (lineActive || !once(node)) {
            return false;
          }
          const [inFrom, inTo] = codeInside(node);
          const code = doc.sliceString(inFrom, inTo);
          const math =
            doc.sliceString(ref.from - 1, ref.from) === "$" &&
            !escapedAt(doc, ref.from - 1) &&
            doc.sliceString(ref.to, ref.to + 1) === "$";
          if (math) {
            out.push(
              Decoration.replace({
                widget: new MathWidget(unpadded(code), false),
              }).range(ref.from - 1, ref.to + 1),
            );
            return false;
          }
          const colour = colourOf(code);
          if (colour !== null) {
            out.push(
              Decoration.widget({
                widget: new SwatchWidget(colour),
                side: -1,
              }).range(ref.from),
            );
          }
          return false;
        }
      },
    });
  }
  out.sort((a, b) => a.from - b.from || a.to - b.to);
  return {
    decorations: Decoration.set(out, true),
    // Only what is replaced: the line tint and the title's weight sit over
    // text a caret must still reach anywhere inside.
    hidden: Decoration.set(
      out.filter((range) => range.from < range.to && range.value.point),
      true,
    ),
  };
}

const inlinePreviews = ViewPlugin.fromClass(
  class {
    built: Inline;

    constructor(view: EditorView) {
      this.built = buildInline(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.viewportChanged ||
        parseAdvanced(update) ||
        parkingChanged(update) ||
        markModeChanged(update)
      ) {
        this.built = buildInline(update.view);
      }
    }
  },
  {
    decorations: (value) => value.built.decorations,
    // Hidden marks are not a place a caret may sit, as in `preview.ts`.
    provide: (plugin) =>
      EditorView.atomicRanges.of(
        (view) => view.plugin(plugin)?.built.hidden ?? Decoration.none,
      ),
  },
);

/** A display formula as the scan found it: where, what and in which callout. */
interface Scanned extends MathBlock {
  callout: CalloutKind | null;
  edge: string;
  /**
   * An unclosed `math` fence (D43): its text stays on the page whatever the
   * caret does, and the formula is drawn after it instead of over it.
   */
  unclosed: boolean;
}

/** Every display formula of the text: a walk of the whole tree. */
function scanBlocks(state: EditorState, sections: boolean): Scanned[] {
  const doc = state.doc;
  const scanned = (block: MathBlock, unclosed: boolean): Scanned => {
    const callout = calloutAt(state, block.from + 1);
    const edge =
      callout === null
        ? ""
        : [
            doc.lineAt(block.from).number ===
            doc.lineAt(callout.quote.from).number
              ? "cm-callout-first"
              : "",
            doc.lineAt(block.to).number === doc.lineAt(callout.quote.to).number
              ? "cm-callout-last"
              : "",
          ]
            .join(" ")
            .trim();
    return { ...block, callout: callout?.callout ?? null, edge, unclosed };
  };
  return [
    ...mathBlocks(state, sections).map((block) => scanned(block, false)),
    ...openMathFences(state, sections).map((block) => scanned(block, true)),
  ].filter((block) => block.tex.trim() !== "");
}

/** Whether any line of a formula shows its source. */
function blockShowsSource(state: EditorState, block: Scanned): boolean {
  if (block.unclosed) {
    return false;
  }
  const first = state.doc.lineAt(block.from).number;
  const last = state.doc.lineAt(block.to).number;
  for (let number = first; number <= last; number += 1) {
    if (showsSource(state, number)) {
      return true;
    }
  }
  return false;
}

/** Which scanned formulas show their source, as one comparable word. */
function touchedKey(state: EditorState, blocks: Scanned[]): string {
  return blocks
    .map((block, index) => (blockShowsSource(state, block) ? index : -1))
    .filter((index) => index !== -1)
    .join(",");
}

/**
 * The formulas drawn: every scanned one the selection does not touch, over
 * its source; an unclosed fence keeps its source and gets the formula after.
 */
function drawBlocks(state: EditorState, blocks: Scanned[]): DecorationSet {
  const touched = new Set(touchedKey(state, blocks).split(","));
  return Decoration.set(
    blocks
      .filter((_block, index) => !touched.has(String(index)))
      .map((block) => {
        const widget = new MathWidget(
          block.tex,
          true,
          block.callout,
          block.edge,
        );
        return block.unclosed
          ? Decoration.widget({ widget, block: true, side: 1 }).range(block.to)
          : Decoration.replace({ widget, block: true }).range(
              block.from,
              block.to,
            );
      }),
    true,
  );
}

interface BlockPreviews {
  blocks: Scanned[];
  touched: string;
  drawn: DecorationSet;
}

function previewsOf(state: EditorState, blocks: Scanned[]): BlockPreviews {
  return {
    blocks,
    touched: touchedKey(state, blocks),
    drawn: drawBlocks(state, blocks),
  };
}

/**
 * The display formulas. The tree is walked when the text or its parse
 * moves; a caret move only asks which formulas the selection touches now,
 * and keeps the drawn set when that answer is the same.
 */
function blockPreviews(sections: boolean): Extension {
  return StateField.define<BlockPreviews>({
    create: (state) => previewsOf(state, scanBlocks(state, sections)),
    update: (value, tr) => {
      if (tr.docChanged || parseAdvanced(tr)) {
        return previewsOf(tr.state, scanBlocks(tr.state, sections));
      }
      if (
        (tr.selection === undefined &&
          !parkingChanged(tr) &&
          !markModeChanged(tr)) ||
        touchedKey(tr.state, value.blocks) === value.touched
      ) {
        return value;
      }
      return previewsOf(tr.state, value.blocks);
    },
    provide: (field) =>
      EditorView.decorations.from(field, (value) => value.drawn),
  });
}

const elementTheme = EditorView.baseTheme({
  ".cm-callout-head": { marginRight: "0.375rem", fontWeight: "600" },
  ".cm-callout-title": { fontWeight: "600" },
  ".cm-math-block": {
    padding: "0.5rem 0.75rem",
    overflowX: "auto",
  },
  // Inside a callout, in from the rule like the callout's words.
  ".cm-math-block.callout": { paddingLeft: "1rem" },
  ".cm-math-block.callout.cm-callout-first": {
    borderTopRightRadius: "0.375rem",
  },
  ".cm-math-block.callout.cm-callout-last": {
    borderBottomRightRadius: "0.375rem",
  },
});

/**
 * The whole layer, inside the preview compartment. `sections` for a buffer
 * whose page is read one level-2 section at a time (a Reflection page).
 */
export function elementPreviews(
  options: { sections?: boolean } = {},
): Extension {
  return [
    mathSections.of(options.sections ?? false),
    inlinePreviews,
    blockPreviews(options.sections ?? false),
    elementTheme,
  ];
}
