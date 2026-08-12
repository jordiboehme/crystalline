/**
 * Live previews below the blocks that earn them: a mermaid fence renders its
 * diagram, a table renders a readable grid. Both are widgets appended AFTER
 * the source they preview - the syntax stays on screen and editable, which
 * is the model's known trade-off, stated in the spec.
 *
 * Mermaid arrives by dynamic import so the editor chunk does not carry the
 * renderer; the module is the same one MarkdownBody lazy-loads, so the
 * bundler serves one copy. It is initialized from the shared configuration in
 * `theme/mermaid`, the one MermaidDiagram uses, so the same fence draws the
 * same diagram in the same palette here and on the page; a diagram that will
 * not parse draws no diagram and one quiet caption instead, carrying the
 * parser's own complaint and the document line it stumbled on - never the
 * author's source echoed back, which is already right above the caption and is
 * where the fix goes.
 *
 * A `StateField` rather than a `ViewPlugin`: CodeMirror refuses block
 * decorations from a plugin's dynamic source outright ("Block decorations
 * may not be specified via plugins"), since a block widget changes line
 * layout and the editor has to know about it before it decides how a line
 * even draws. The field reads the whole document's syntax tree rather than
 * `view.visibleRanges` because a state field has no view to ask; the same
 * node-offset positions come out either way.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import { StateField } from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";

import { mermaidConfig } from "../theme/mermaid";

let mermaidSequence = 0;

/** What a failure says when the message names nothing a person can act on. */
const UNRENDERABLE = "This diagram does not render yet.";

/** One line under a fence, not a transcript: one captured message ran to 740. */
const CAPTION_CAP = 160;

/**
 * The caret ruler jison draws under the source it echoes back, and the ONLY
 * structural marker in any of these messages. The echo itself is the author's
 * own text and could be anything, so it is recognized by the ruler that always
 * follows it rather than by anything about the line itself.
 */
const CARET_RULER = /^[-\s]*\^[-\s]*$/;

/**
 * Mermaid's own lead-in, and the only place a line number is allowed to come
 * from. Both grammars write one: jison says `Parse error on line 3:` or
 * `Lexical error on line 3.`, langium says `Parsing failed:  Parse error on
 * line 2, column 11:` and sometimes `Lexer error on line 4, column 9:`, with
 * `?` where it cannot place the failure. Anchored, because
 * `UnknownDiagramError` carries the whole fence body in its message and an
 * unanchored search would read a number out of the author's own prose.
 */
const ERROR_LEAD =
  /^.*?(?:parse|lexer|lexical) error on line (\d+|\?)(?:,\s*column\s*(?:\d+|\?))?\s*[.:]?\s*/i;

/**
 * `Expecting <tokens>, got <token>`: jison's shape, and the one message worth
 * shortening in the middle rather than at the end (see `fit`).
 */
const EXPECTING = /^(Expecting )(.*)(, got .+)$/i;

/** The family that quotes the fence body back; never worth showing. */
const UNDETECTED = /^No diagram type detected/i;

/** Where a fence's body sits in the document, so a caption can say it. */
export interface FenceBody {
  /** Document line number (1-based) of the body's first line. */
  firstLine: number;
  /** How many lines the body has. */
  lineCount: number;
}

/**
 * The message as one line, with jison's echo of the author's source removed.
 *
 * A jison failure is four lines - lead-in, the author's own source, a caret
 * ruler, then the complaint - and only the last one says anything the source
 * above the caption does not already say. A langium failure is one line that
 * may contain a raw newline, because the token it found can BE a newline.
 * Collapsing what survives into a single spaced line serves both.
 */
function condense(message: string): string {
  const lines = message.split(/\r?\n/);
  const kept = lines.filter((line, index) => {
    const next = lines[index + 1];
    const echo = next !== undefined && CARET_RULER.test(next);
    return !CARET_RULER.test(line) && !echo;
  });
  return kept.join(" ").replace(/\s+/g, " ").trim();
}

/** The caption in code points, so no cut ever lands inside a surrogate pair. */
function fit(detail: string, budget: number): string {
  const points = Array.from(detail);
  if (points.length <= budget) {
    return detail;
  }
  const shape = EXPECTING.exec(detail);
  if (shape) {
    // `Expecting 'A', 'B', ... 'Z', got 'X'` - both ends carry the meaning and
    // the tail carries most of it, so the token list gives way in the middle
    // rather than the sentence losing the token it actually choked on.
    const head = shape[1] ?? "";
    const tail = ` ... ${(shape[3] ?? "").replace(/^,\s*/, "")}`;
    const room = budget - Array.from(head).length - Array.from(tail).length;
    const tokens: string[] = [];
    let used = 0;
    for (const token of (shape[2] ?? "").split(", ")) {
      const width = Array.from(token).length + (tokens.length === 0 ? 0 : 2);
      if (used + width > room) {
        break;
      }
      tokens.push(token);
      used += width;
    }
    if (tokens.length > 0) {
      return `${head}${tokens.join(", ")}${tail}`;
    }
  }
  return `${points.slice(0, budget - 3).join("")}...`;
}

/**
 * A rejected render, in one muted line: `Line 7: Expecting 'TAGEND', 'STR',
 * 'MD_STR', got 'SQS'`.
 *
 * Pure, and exported for its own tests: every wording decision is made here
 * and the widget only appends what comes back. The tests hold captured
 * messages rather than plausible ones, because every rule in here looks
 * correct against a message somebody made up.
 *
 * `Line N` is a DOCUMENT line, which is why the fence's own position travels
 * in. Mermaid counts inside the fence body, and `FindingsPanel` says "Go to
 * line N" about the document a few centimetres away from this caption, so a
 * fence-relative number would be a second silent meaning of the same word. The
 * number is also clamped into the body: an unterminated construct - the state
 * a diagram is in on nearly every keystroke - makes the parser run to the end
 * of the text and report one line past it.
 *
 * A message with no line is not thrown away. The ones carrying a line are
 * jison's machine text; the ones without are usually the sentences a mermaid
 * maintainer wrote for a person ("Trying to inactivate an inactive
 * participant (Alice)"), which are the most useful captions in the corpus. The
 * exception is `UnknownDiagramError`, which quotes the entire fence body back
 * and is what every fence throws until its first word parses.
 *
 * A cause that is not an `Error` is an ordinary path rather than a defensive
 * one - the same `.catch` also catches a failed dynamic import - so anything
 * without a readable message answers with the plain sentence rather than
 * `[object Object]`.
 */
export function describeMermaidError(cause: unknown, fence: FenceBody): string {
  const raw =
    cause instanceof Error
      ? cause.message
      : typeof cause === "string"
        ? cause
        : "";
  const message = condense(raw);
  const lead = ERROR_LEAD.exec(message);
  const detail = (lead ? message.slice(lead[0].length) : message)
    .replace(/^[,.:;]+\s*/, "")
    .trim();
  const reported = lead?.[1];
  if (reported === undefined || !/^\d+$/.test(reported)) {
    return detail.length === 0 || UNDETECTED.test(detail)
      ? UNRENDERABLE
      : fit(detail, CAPTION_CAP);
  }
  const within = Math.min(Math.max(Number(reported), 1), fence.lineCount);
  const prefix = `Line ${String(fence.firstLine + within - 1)}: `;
  return `${prefix}${fit(detail.length > 0 ? detail : UNRENDERABLE, CAPTION_CAP - prefix.length)}`;
}

class MermaidPreviewWidget extends WidgetType {
  // Assigned in the body rather than declared as constructor parameters: the
  // build erases types, it does not run a TypeScript transform, and
  // parameter properties are syntax that would need one.
  readonly source: string;
  readonly dark: boolean;
  /** Document line the fence body starts on, for the caption's `Line N`. */
  readonly firstLine: number;

  constructor(source: string, dark: boolean, firstLine: number) {
    super();
    this.source = source;
    this.dark = dark;
    this.firstLine = firstLine;
  }

  // The fence's own position is part of what makes two widgets equal, because
  // the caption asserts a document line: a widget whose source is unchanged
  // but which has been pushed down the document has to draw again, or it keeps
  // naming the line it used to sit on. The cost is one re-render of a diagram
  // when lines are added or removed ABOVE it, which is a line-count change
  // rather than a keystroke, so typing near a diagram does not redraw it.
  override eq(other: MermaidPreviewWidget): boolean {
    return (
      other.source === this.source &&
      other.dark === this.dark &&
      other.firstLine === this.firstLine
    );
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("div");
    box.className = "cm-mermaid-preview";
    const { source, dark, firstLine } = this;
    mermaidSequence += 1;
    const id = `cm-mermaid-${String(mermaidSequence)}`;
    void import("mermaid")
      .then(async ({ default: mermaid }) => {
        // The same configuration the reading view uses, so a diagram does
        // not change palette between the editor and the page. It carries
        // `suppressErrorRendering`, which this surface needs most: the
        // preview redraws on every keystroke, so most of what it asks
        // mermaid to draw is a half-typed diagram that fails, and without
        // that flag each failure appends mermaid's error graphic to
        // `document.body` - outside the editor, outside CodeMirror's own
        // teardown - where they accumulate for the whole session. A broken
        // diagram draws no diagram; what it draws instead is the caption
        // below, and the source is right above it.
        mermaid.initialize(mermaidConfig(dark));
        const rendered = await mermaid.render(id, source);
        if (box.isConnected || box.childElementCount === 0) {
          box.innerHTML = rendered.svg;
        }
      })
      .catch((cause: unknown) => {
        // Only in place of a blank, never over a diagram. The guard is
        // structural rather than careful: every keystroke builds a NEW
        // widget whose box starts empty, and the only thing that ever puts
        // a child in it is the resolved branch above, so an empty box here
        // means this render drew nothing at all. A widget that did draw and
        // is later replaced takes its own SVG down with it. No `isConnected`
        // clause, unlike the resolved branch: a box that has already left the
        // screen is about to be garbage, and a caption written into it costs
        // one element nobody sees, where writing an SVG into a live box that
        // has moved on would show the wrong diagram.
        if (box.childElementCount > 0) {
          return;
        }
        const caption = document.createElement("div");
        caption.className = "cm-mermaid-error";
        // Text, never markup, and no live region: this fires on most
        // keystrokes while a diagram is being typed, so a screen reader
        // announcing each one would talk over the typing.
        caption.textContent = describeMermaidError(cause, {
          firstLine,
          lineCount: source.split("\n").length,
        });
        box.appendChild(caption);
      });
    return box;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

/** A pipe table, parsed just far enough to draw: cells are text, never HTML. */
class TablePreviewWidget extends WidgetType {
  readonly source: string;

  constructor(source: string) {
    super();
    this.source = source;
  }

  override eq(other: TablePreviewWidget): boolean {
    return other.source === this.source;
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("div");
    box.className = "cm-table-preview";
    const rows = this.source
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.startsWith("|"))
      .map((line) =>
        line
          .replace(/^\||\|$/g, "")
          .split("|")
          .map((cell) => cell.trim()),
      );
    const table = document.createElement("table");
    rows.forEach((cells, index) => {
      if (index === 1 && cells.every((cell) => /^:?-+:?$/.test(cell))) {
        return; // the alignment row draws nothing
      }
      const row = document.createElement("tr");
      for (const cell of cells) {
        const element = document.createElement(index === 0 ? "th" : "td");
        element.textContent = cell;
        row.appendChild(element);
      }
      table.appendChild(row);
    });
    box.appendChild(table);
    return box;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

function buildPreviews(state: EditorState, dark: boolean): DecorationSet {
  const widgets: Range<Decoration>[] = [];
  const doc = state.doc;
  syntaxTree(state).iterate({
    enter: (node) => {
      if (node.name === "FencedCode") {
        const text = doc.sliceString(node.from, node.to);
        const fence = /^```(\w+)?[^\n]*\n([\s\S]*?)\n?```\s*$/.exec(text);
        if (fence && fence[1] === "mermaid") {
          widgets.push(
            Decoration.widget({
              // The body starts on the line after the opening delimiter, which
              // the fence pattern guarantees is a line of its own. That number
              // is what turns mermaid's fence-relative line into a line of
              // this document.
              widget: new MermaidPreviewWidget(
                fence[2] ?? "",
                dark,
                doc.lineAt(node.from).number + 1,
              ),
              block: true,
              side: 1,
            }).range(node.to),
          );
        }
      }
      if (node.name === "Table") {
        widgets.push(
          Decoration.widget({
            widget: new TablePreviewWidget(doc.sliceString(node.from, node.to)),
            block: true,
            side: 1,
          }).range(node.to),
        );
      }
    },
  });
  return Decoration.set(widgets.sort((a, b) => a.from - b.from));
}

/**
 * The previews' own chrome.
 *
 * `.cm-mermaid-error` is a fresh class with no competing rule anywhere - not
 * a `.cm-line`, not the scroller, written by this module alone - so cascade
 * trap 1 (base style modules mount REVERSED, and equal-specificity rules are
 * decided by that order) has nothing to bite on and a plain entry is safe. If
 * a competitor ever appears, the fallback is `fenceMono`'s move: write the
 * rule one class deeper (`.cm-mermaid-preview .cm-mermaid-error`) so it wins
 * on specificity whatever the mount order turns out to be.
 *
 * Muted slate at the caption step, never red: mid-typing a diagram is
 * "broken" almost continuously, so this is a hint about a blank space, not an
 * alarm. The `&dark` variant is rewritten to carry the view's dark scope
 * class, which makes it one class deeper than the plain rule and so a
 * specificity win rather than an order win; being emitted second only settles
 * a tie that does not arise.
 */
const previewTheme = EditorView.baseTheme({
  ".cm-mermaid-preview": { padding: "0.5rem 0.75rem" },
  ".cm-mermaid-error": {
    color: "var(--color-slate-500)",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
  },
  "&dark .cm-mermaid-error": { color: "var(--color-slate-400)" },
  ".cm-table-preview": { padding: "0.25rem 0.75rem" },
  ".cm-table-preview table": { borderCollapse: "collapse" },
  ".cm-table-preview th, .cm-table-preview td": {
    border: "1px solid var(--color-slate-300)",
    padding: "0.15rem 0.5rem",
    textAlign: "left",
  },
});

/** Mermaid and table previews. `dark` picks the mermaid theme. */
export function fencePreviews(dark: boolean): Extension {
  const field = StateField.define<DecorationSet>({
    create: (state) => buildPreviews(state, dark),
    update: (value, tr) =>
      tr.docChanged ? buildPreviews(tr.state, dark) : value,
    provide: (f) => EditorView.decorations.from(f),
  });
  return [field, previewTheme];
}
