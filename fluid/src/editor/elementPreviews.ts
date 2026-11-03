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
import type { ReactElement } from "react";
import { createElement } from "react";
import type { Root } from "react-dom/client";
import { createRoot } from "react-dom/client";

import { CALLOUT_ICONS, CALLOUT_WORDS } from "../components/calloutFace";
import { lazyPart } from "../components/lazyPart";
import type { CalloutKind } from "../markdown/callouts";
import type { StatusColour } from "../markdown/directives";
import { labelPlainWords } from "../markdown/statusText";
import { SWATCH_CLASSES, colourOf } from "../markdown/swatch";
import { calloutAt, calloutOf, statusOf } from "./elementContext";
import { frontmatterRegion } from "./frontmatterRegion";
import { markModeChanged, marksHiddenOnLine } from "./marks/mode";
import { parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";
import { CODE_CONTEXTS } from "./prose";

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

  override toDOM(): HTMLElement {
    const dom = document.createElement("span");
    dom.className = "cm-callout-head";
    const Icon = CALLOUT_ICONS[this.kind];
    const icon = {
      "aria-hidden": true,
      size: 16,
      className: "shrink-0 text-(--callout)",
    };
    draw(
      dom,
      createElement(
        "span",
        { className: "inline-flex items-center gap-1.5 align-[-0.15em]" },
        // The page's head: a chevron when it folds, the kind's icon unless
        // the kind is the fold itself, the word when nothing else names it.
        this.fold === null
          ? null
          : createElement(ChevronRight, {
              ...icon,
              key: "fold",
              className: `${icon.className} ${this.fold === "open" ? "rotate-90" : ""}`,
            }),
        this.fold !== null && this.kind === "details"
          ? null
          : createElement(Icon, { ...icon, key: "kind" }),
        this.titled
          ? null
          : createElement("span", { key: "word" }, CALLOUT_WORDS[this.kind]),
      ),
    );
    return dom;
  }

  override destroy(dom: HTMLElement): void {
    undraw(dom);
  }

  override ignoreEvent(): boolean {
    // A click is a click into the text: the caret lands on the head's line
    // and the marker shows as written.
    return false;
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

  override toDOM(): HTMLElement {
    const pill = document.createElement("span");
    pill.className = "status-label";
    pill.dataset["status"] = this.colour;
    pill.textContent = this.label;
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
    const dom = document.createElement(this.display ? "div" : "span");
    dom.className = this.display ? "cm-math-block" : "cm-math-inline";
    if (this.callout !== null) {
      dom.classList.add("callout");
      dom.dataset["callout"] = this.callout;
      for (const name of this.edge.split(" ").filter(Boolean)) {
        dom.classList.add(name);
      }
    }
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
                  doc.sliceString(open.to, close.from),
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
            doc.sliceString(ref.from - 2, ref.from - 1) !== "\\" &&
            doc.sliceString(ref.to, ref.to + 1) === "$";
          if (math) {
            out.push(
              Decoration.replace({
                widget: new MathWidget(code, false),
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

/** The source and the span of one display formula. */
interface Block {
  from: number;
  to: number;
  tex: string;
}

/** A line's quote marks: how many, and how long the run of them is. */
const QUOTE_RUN = /^[ \t]*((?:>[ \t]?)*)/;

function quoteDepth(text: string): number {
  return (QUOTE_RUN.exec(text)?.[1] ?? "").split(">").length - 1;
}

/** `text` without its first `depth` quote marks (and a space after each). */
function unquoted(text: string, depth: number): string {
  let rest = text;
  for (let mark = 0; mark < depth; mark += 1) {
    const prefix = /^[ \t]*>[ \t]?/.exec(rest);
    if (prefix === null) {
      break;
    }
    rest = rest.slice(prefix[0].length);
  }
  return rest;
}

/** A page section's heading, which ends anything a section holds. */
const SECTION_HEADING = /^##(?:[ \t]|$)/;

/**
 * A `math` fence and a `$$` block, each with its formula, outside code. Both
 * may stand in a quote: their lines are read without the quote's marks, and
 * a `$$` closes only at the depth it opened, inside the same quote. With
 * `sections`, a section heading ends an open `$$`, because each section of
 * the page is read on its own.
 */
function mathBlocks(state: EditorState, sections: boolean): Block[] {
  const doc = state.doc;
  const blocks: Block[] = [];
  const code: { from: number; to: number }[] = [];
  syntaxTree(state).iterate({
    enter: (node) => {
      if (!CODE_CONTEXTS.has(node.name) || node.name === "InlineCode") {
        return;
      }
      code.push({ from: node.from, to: node.to });
      if (node.name !== "FencedCode") {
        return false;
      }
      const first = doc.lineAt(node.from);
      const last = doc.lineAt(node.to);
      const depth = quoteDepth(first.text);
      const lines: string[] = [];
      for (let number = first.number; number <= last.number; number += 1) {
        lines.push(unquoted(doc.line(number).text, depth));
      }
      // A fence that does not start its line (one in a list item) stays
      // source: its block would have to start mid-line.
      const fence = /^```math[ \t]*\n([\s\S]*?)\n?```[ \t]*$/.exec(
        lines.join("\n"),
      );
      // On a page read section by section, a fence that runs over a section
      // heading is not one block on the page: draw none.
      const crosses =
        sections &&
        lines.some(
          (text, index) =>
            index > 0 &&
            quoteDepth(doc.line(first.number + index).text) === 0 &&
            SECTION_HEADING.test(text),
        );
      if (fence !== null && !crosses) {
        blocks.push({ from: first.from, to: last.to, tex: fence[1] ?? "" });
      }
      return false;
    },
  });
  const fmEnd = frontmatterRegion(doc)?.to ?? -1;
  const inCode = (pos: number) =>
    pos <= fmEnd || code.some((range) => pos >= range.from && pos <= range.to);
  let open: { number: number; depth: number } | null = null;
  for (let number = 1; number <= doc.lines; number += 1) {
    const line = doc.line(number);
    if (inCode(line.from)) {
      continue;
    }
    const depth = quoteDepth(line.text);
    const rest = unquoted(line.text, depth);
    if (open !== null && depth < open.depth) {
      // The quote the block opened in has ended.
      open = null;
    }
    if (
      open !== null &&
      sections &&
      depth === 0 &&
      SECTION_HEADING.test(rest)
    ) {
      open = null;
      continue;
    }
    if (rest.trim() !== "$$") {
      continue;
    }
    if (open !== null && depth > open.depth) {
      // A deeper line is the formula's content, as the reader reads it.
      continue;
    }
    if (open === null) {
      open = { number, depth };
      continue;
    }
    const tex: string[] = [];
    for (let inner = open.number + 1; inner < number; inner += 1) {
      tex.push(unquoted(doc.line(inner).text, depth));
    }
    blocks.push({
      from: doc.line(open.number).from,
      to: line.to,
      tex: tex.join("\n"),
    });
    open = null;
  }
  return blocks;
}

/** A display formula as the scan found it: where, what and in which callout. */
interface Scanned extends Block {
  callout: CalloutKind | null;
  edge: string;
}

/** Every display formula of the text: a walk of the whole tree. */
function scanBlocks(state: EditorState, sections: boolean): Scanned[] {
  const doc = state.doc;
  return mathBlocks(state, sections)
    .filter((block) => block.tex.trim() !== "")
    .map((block) => {
      const callout = calloutAt(state, block.from + 1);
      const edge =
        callout === null
          ? ""
          : [
              doc.lineAt(block.from).number ===
              doc.lineAt(callout.quote.from).number
                ? "cm-callout-first"
                : "",
              doc.lineAt(block.to).number ===
              doc.lineAt(callout.quote.to).number
                ? "cm-callout-last"
                : "",
            ]
              .join(" ")
              .trim();
      return { ...block, callout: callout?.callout ?? null, edge };
    });
}

/** Whether any line of a formula shows its source. */
function blockShowsSource(state: EditorState, block: Scanned): boolean {
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

/** The formulas drawn: every scanned one the selection does not touch. */
function drawBlocks(state: EditorState, blocks: Scanned[]): DecorationSet {
  const touched = new Set(touchedKey(state, blocks).split(","));
  return Decoration.set(
    blocks
      .filter((_block, index) => !touched.has(String(index)))
      .map((block) =>
        Decoration.replace({
          widget: new MathWidget(block.tex, true, block.callout, block.edge),
          block: true,
        }).range(block.from, block.to),
      )
      .sort((a, b) => a.from - b.from),
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
    inlinePreviews,
    blockPreviews(options.sections ?? false),
    elementTheme,
  ];
}
