/**
 * A static grid cell, drawn from its Markdown.
 *
 * The cell's text goes through the editor's own inline parser - the one the
 * table parser runs on every cell, with GFM and the dialect's status labels
 * and math - and every node it knows is drawn as an element built here:
 * emphasis, strikethrough, code, a link, a wikilink chip, a status pill,
 * inline math and a `<br>`. Everything else, other HTML included, is text.
 * Nothing is ever set as HTML.
 *
 * Escapes are drawn as the character they escape, the way Markdown renders
 * them, so `\|` is `|` and `\\` is `\`; the cell editor shows the same.
 */

import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import type { NodeSet } from "@lezer/common";

import { isWebAddress } from "../../format";
import { statusColour } from "../../markdown/directives";
import { labelPlainWords, statusPieces } from "../../markdown/statusText";
import type { WikilinkResolver } from "../../wikilinks";
import { referenceState, wikilinkWords } from "../../wikilinks";
import { dialectGrammar } from "../dialectHighlight";

export interface CellRenderOptions {
  /** What a wikilink chip asks about its target; none draws it pending. */
  resolve?: WikilinkResolver;
}

/**
 * One inline element as the parser builds it. `children` is internal to
 * `@lezer/markdown` (its `Element` declares only `type`, `from` and `to`), but
 * `parseInline` hands back nothing else to read the nesting from, and a cell
 * is inline content by definition: it is what the table parser itself does
 * with each cell.
 */
interface InlineElement {
  readonly type: number;
  readonly from: number;
  readonly to: number;
  readonly children?: readonly InlineElement[];
}

interface InlineParser {
  readonly nodeSet: NodeSet;
  parseInline(text: string, offset: number): InlineElement[];
}

/** The parts of the inline context the wikilink parser uses. */
interface Inline {
  readonly end: number;
  char(pos: number): number;
  slice(from: number, to: number): string;
  elt(type: string, from: number, to: number): unknown;
  addElement(element: unknown): number;
}

const OPEN_BRACKET = 91;
const WIKILINK_AT = /^\[\[([^[\]]+)\]\]/;

/**
 * `[[Target]]` as a node of its own, read before a link so the brackets are
 * not taken for one. Code that starts first stays code.
 */
const wikilinkGrammar = {
  defineNodes: [{ name: "Wikilink" }],
  parseInline: [
    {
      name: "Wikilink",
      before: "Link",
      parse(cx: Inline, next: number, pos: number): number {
        if (next !== OPEN_BRACKET || cx.char(pos + 1) !== OPEN_BRACKET) {
          return -1;
        }
        const match = WIKILINK_AT.exec(cx.slice(pos, cx.end));
        if (
          match === null ||
          wikilinkWords(unescapePipes(match[1] ?? "")) === null
        ) {
          return -1;
        }
        return cx.addElement(cx.elt("Wikilink", pos, pos + match[0].length));
      },
    },
  ],
};

let parser: InlineParser | null = null;

function inlineParser(): InlineParser {
  parser ??= markdown({
    base: markdownLanguage,
    extensions: [dialectGrammar, wikilinkGrammar],
  }).language.parser as unknown as InlineParser;
  return parser;
}

/** A pipe is escaped inside a table cell; the target names it bare. */
function unescapePipes(text: string): string {
  return text.replaceAll("\\|", "|");
}

const BREAK_TAG = /^<br\s*\/?>$/i;
const MARKS = new Set([
  "EmphasisMark",
  "StrikethroughMark",
  "CodeMark",
  "LinkMark",
  "MathMark",
  "StatusMark",
]);

type Katex = typeof import("katex").default;
let katexLoad: Promise<Katex> | null = null;

/** KaTeX and its stylesheet, asked for once the first cell holds math. */
function loadKatex(): Promise<Katex> {
  katexLoad ??= Promise.all([
    import("katex"),
    import("katex/dist/katex.min.css"),
  ]).then(([module]) => module.default);
  return katexLoad;
}

/** Which draw of a math element is the current one, so a late render skips a stale one. */
const mathDraws = new WeakMap<HTMLElement, number>();
let mathDrawCount = 0;

function drawMath(tex: string, into: Node): void {
  const span = document.createElement("span");
  span.className = "cm-math-inline";
  // The TeX as code until KaTeX arrives, as the main view shows it.
  const code = document.createElement("code");
  code.textContent = tex;
  span.appendChild(code);
  into.appendChild(span);
  mathDrawCount += 1;
  const draw = mathDrawCount;
  mathDraws.set(span, draw);
  void loadKatex().then(
    (katex) => {
      if (mathDraws.get(span) !== draw || !span.isConnected) return;
      try {
        katex.render(tex, span, {
          displayMode: false,
          throwOnError: false,
          strict: "ignore",
          trust: false,
        });
      } catch {
        span.replaceChildren(code);
      }
    },
    () => {
      // Offline: the TeX stays, which is what was written.
    },
  );
}

/** Lets a plain click open the cell; Mod-click opens the link as usual. */
function holdNavigation(event: MouseEvent): void {
  if (!(event.metaKey || event.ctrlKey)) event.preventDefault();
}

class Drawer {
  readonly text: string;
  readonly nodes: NodeSet;
  readonly options: CellRenderOptions;

  constructor(text: string, nodes: NodeSet, options: CellRenderOptions) {
    this.text = text;
    this.nodes = nodes;
    this.options = options;
  }

  name(element: InlineElement): string {
    return this.nodes.types[element.type]?.name ?? "";
  }

  plain(from: number, to: number, into: Node): void {
    if (to > from)
      into.appendChild(document.createTextNode(this.text.slice(from, to)));
  }

  /**
   * `[from, to)` drawn into `into`, with `elements` in it. A `$` right
   * around inline code makes it math, as the main view reads it.
   */
  range(
    elements: readonly InlineElement[],
    from: number,
    to: number,
    into: Node,
  ): void {
    let at = from;
    for (let i = 0; i < elements.length; i += 1) {
      const element = elements[i];
      if (!element || element.from < at) continue;
      const name = this.name(element);
      if (name === "InlineCode" && this.dollarMath(element, at, to)) {
        this.plain(at, element.from - 1, into);
        drawMath(this.codeText(element), into);
        at = element.to + 1;
        continue;
      }
      this.plain(at, element.from, into);
      if (!MARKS.has(name)) this.element(element, name, into);
      at = element.to;
    }
    this.plain(at, to, into);
  }

  dollarMath(element: InlineElement, at: number, to: number): boolean {
    const text = this.text;
    return (
      element.from - 1 >= at &&
      text[element.from - 1] === "$" &&
      text[element.from - 2] !== "\\" &&
      element.to < to &&
      text[element.to] === "$"
    );
  }

  children(element: InlineElement): readonly InlineElement[] {
    return element.children ?? [];
  }

  /** The text between the first and the last mark child. */
  inner(element: InlineElement): { from: number; to: number } {
    const kids = this.children(element);
    const first = kids[0];
    const last = kids.at(-1);
    const from = first && MARKS.has(this.name(first)) ? first.to : element.from;
    const to =
      last && last !== first && MARKS.has(this.name(last))
        ? last.from
        : element.to;
    return { from, to: Math.max(from, to) };
  }

  codeText(element: InlineElement): string {
    const { from, to } = this.inner(element);
    return this.text.slice(from, to);
  }

  wrap(tag: string, element: InlineElement, into: Node): void {
    const box = document.createElement(tag);
    const { from, to } = this.inner(element);
    this.range(this.children(element), from, to, box);
    into.appendChild(box);
  }

  element(element: InlineElement, name: string, into: Node): void {
    const source = () => this.text.slice(element.from, element.to);
    switch (name) {
      case "Emphasis":
        this.wrap("em", element, into);
        return;
      case "StrongEmphasis":
        this.wrap("strong", element, into);
        return;
      case "Strikethrough":
        this.wrap("s", element, into);
        return;
      case "InlineCode": {
        const code = document.createElement("code");
        code.textContent = this.codeText(element);
        into.appendChild(code);
        return;
      }
      case "InlineMath":
        drawMath(this.codeText(element), into);
        return;
      case "Escape":
        this.plain(element.from + 1, element.to, into);
        return;
      case "HTMLTag":
        if (BREAK_TAG.test(source()))
          into.appendChild(document.createElement("br"));
        else this.plain(element.from, element.to, into);
        return;
      case "Link":
        this.link(element, into);
        return;
      case "URL":
        this.anchor(source(), element.from, element.to, [], into);
        return;
      case "Autolink": {
        const url = this.children(element).find(
          (kid) => this.name(kid) === "URL",
        );
        if (url)
          this.anchor(
            this.text.slice(url.from, url.to),
            url.from,
            url.to,
            [],
            into,
          );
        else this.plain(element.from, element.to, into);
        return;
      }
      case "Wikilink":
        this.wikilink(source(), into);
        return;
      case "StatusLabel":
        this.status(source(), into);
        return;
      default:
        this.plain(element.from, element.to, into);
    }
  }

  /** `[words](url)` with a web address is an anchor; any other link its text. */
  link(element: InlineElement, into: Node): void {
    const kids = this.children(element);
    const marks = kids.filter((kid) => this.name(kid) === "LinkMark");
    const url = kids.find((kid) => this.name(kid) === "URL");
    const open = marks[0];
    const close = marks[1];
    if (!url || !open || !close) {
      this.plain(element.from, element.to, into);
      return;
    }
    const words = kids.filter(
      (kid) => kid.from >= open.to && kid.to <= close.from,
    );
    const href = this.text.slice(url.from, url.to);
    if (!isWebAddress(href)) {
      // The words stay, the address goes: nothing that runs on press.
      this.range(words, open.to, close.from, into);
      return;
    }
    this.anchor(href, open.to, close.from, words, into);
  }

  anchor(
    href: string,
    from: number,
    to: number,
    words: readonly InlineElement[],
    into: Node,
  ): void {
    if (!isWebAddress(href)) {
      this.range(words, from, to, into);
      return;
    }
    const anchor = document.createElement("a");
    anchor.href = href;
    anchor.rel = "noopener noreferrer";
    anchor.target = "_blank";
    anchor.addEventListener("click", holdNavigation);
    this.range(words, from, to, anchor);
    into.appendChild(anchor);
  }

  /** The chip `wikilinkChips` draws, in the same classes. */
  wikilink(source: string, into: Node): void {
    const inner = unescapePipes(source.slice(2, -2));
    const resolution = this.options.resolve?.(inner) ?? null;
    const chip = document.createElement("span");
    chip.className = `cm-wikilink cm-wikilink-${referenceState(resolution, null)}`;
    chip.textContent = wikilinkWords(inner) ?? inner;
    chip.title = `[[${inner}]]`;
    into.appendChild(chip);
  }

  /** The `status-label` pill, exactly as the table preview draws it. */
  status(source: string, into: Node): void {
    for (const piece of statusPieces(source)) {
      if ("text" in piece) {
        into.appendChild(document.createTextNode(piece.text));
      } else {
        const pill = document.createElement("span");
        pill.className = "status-label";
        pill.dataset["status"] = statusColour(piece.braces);
        pill.textContent = labelPlainWords(piece.label);
        into.appendChild(pill);
      }
    }
  }
}

/**
 * Draws one cell's Markdown into `target`, replacing what was there. The
 * text is the cell's content as stored: pipes escaped, `<br>` for a line
 * break.
 */
export function renderCellContent(
  text: string,
  target: HTMLElement,
  options: CellRenderOptions = {},
): void {
  target.replaceChildren();
  if (text === "") return;
  const inline = inlineParser();
  const elements = inline.parseInline(text, 0);
  new Drawer(text, inline.nodeSet, options).range(
    elements,
    0,
    text.length,
    target,
  );
}
