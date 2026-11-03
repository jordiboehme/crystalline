/**
 * Footnotes drawn as numbers. A reference `[^label]` becomes a small raised
 * button showing its number, a definition line `[^label]: text` shows the
 * number in place of `[^label]:` and its space, and a click or Enter on a
 * reference moves the caret to the definition's text. The text stays plain
 * Markdown: every widget is a view over bytes that never change, and a copy
 * over a drawn reference gives the source.
 *
 * The numbers are the reading page's, which comes from remark-gfm and
 * mdast-util-to-hast: a footnote is numbered by the order of its first
 * reference in the body, then the references inside the notes in the order
 * the notes are numbered; labels match without regard to case or the
 * whitespace around them; a reference with no definition is plain text; a
 * definition nobody refers to is not on the page, so it draws as its text;
 * and of two definitions for one label the first is the note.
 *
 * What is a reference and what is code comes from the syntax tree (fences,
 * inline code, math and HTML comments hold no footnotes); the labels follow
 * the footnote grammar. The model is kept in a state field and is scanned
 * again only for an edit that can change it: a keystroke in a line with no
 * `[^`, no code or comment character and no line break leaves it as it is
 * and only moves the positions.
 */

import { syntaxTree } from "@codemirror/language";
import type { ChangeSet, EditorState, Extension } from "@codemirror/state";
import { EditorSelection, StateField } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";

import { frontmatterRegion } from "./frontmatterRegion";
import { markSegment } from "./marks/caret";
import {
  markModeChanged,
  marksHiddenOnLine,
  marksOnEditLine,
} from "./marks/mode";
import { parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";
import { CODE_CONTEXTS } from "./prose";

/** A reference that gets a number. */
interface Ref {
  from: number;
  to: number;
  key: string;
}

/** The note a label names: the span of `[^label]:` and its space. */
interface Note {
  key: string;
  from: number;
  /** Where the note's text starts, after the colon and one space. */
  textFrom: number;
}

interface Model {
  refs: readonly Ref[];
  notes: readonly Note[];
  numbers: ReadonlyMap<string, number>;
  /** The length of the syntax tree the model was read from. */
  parsed: number;
}

let scans = 0;

/** How many whole-document scans have run, for the cost tests. */
export function footnoteScans(): number {
  return scans;
}

/** Subtrees whose text holds no footnote. */
const NO_NOTES = new Set([
  ...CODE_CONTEXTS,
  "InlineMath",
  "Comment",
  "CommentBlock",
  "HTMLBlock",
  "ProcessingInstruction",
]);

/** A label as the footnote grammar matches it to a definition. */
function labelKey(label: string): string {
  return label
    .replace(/[\t\n\r ]+/g, " ")
    .replace(/^ | $/g, "")
    .toLowerCase()
    .toUpperCase();
}

/** Where the line before a definition's `[` may stand: quote marks, up to three spaces. */
const DEFINITION_LEAD = /^ {0,3}(?:> ?)* {0,3}$/;

/** The ranges of `state` that hold no footnote, sorted and apart. */
function excludedRanges(state: EditorState): [number, number][] {
  const out: [number, number][] = [];
  const region = frontmatterRegion(state.doc);
  if (region) {
    out.push([0, region.to]);
  }
  syntaxTree(state).iterate({
    enter: (node) => {
      if (NO_NOTES.has(node.name)) {
        out.push([node.from, node.to]);
        return false;
      }
      return undefined;
    },
  });
  return out.sort((a, b) => a[0] - b[0]);
}

function excluded(ranges: readonly [number, number][], pos: number): boolean {
  let low = 0;
  let high = ranges.length - 1;
  while (low <= high) {
    const mid = (low + high) >> 1;
    const range = ranges[mid];
    if (!range) {
      return false;
    }
    if (pos < range[0]) {
      high = mid - 1;
    } else if (pos >= range[1]) {
      low = mid + 1;
    } else {
      return true;
    }
  }
  return false;
}

/** The label starting at `at` (just after `[^`) and the index of its `]`. */
function labelAt(
  text: string,
  at: number,
): { label: string; end: number } | null {
  let index = at;
  while (index < text.length && index - at <= 999) {
    const char = text[index];
    if (char === "]") {
      return index > at ? { label: text.slice(at, index), end: index } : null;
    }
    if (char === undefined || char === "[" || /\s/.test(char)) {
      return null;
    }
    // A backslash may escape a bracket or another backslash.
    index += char === "\\" && /[[\\\]]/.test(text[index + 1] ?? "") ? 2 : 1;
  }
  return null;
}

function scan(state: EditorState): Model {
  scans += 1;
  const doc = state.doc;
  const text = doc.toString();
  const none = excludedRanges(state);
  interface Found {
    from: number;
    to: number;
    key: string;
    /** For a definition: where its text starts. */
    textFrom?: number;
  }
  const refs: Found[] = [];
  const defs: Found[] = [];
  let at = text.indexOf("[^");
  while (at !== -1) {
    const next = text.indexOf("[^", at + 2);
    if (!excluded(none, at)) {
      let slashes = 0;
      while (text[at - slashes - 1] === "\\") {
        slashes += 1;
      }
      const found = slashes % 2 === 0 ? labelAt(text, at + 2) : null;
      if (found) {
        const to = found.end + 1;
        const key = labelKey(found.label);
        const lineStart = text.lastIndexOf("\n", at - 1) + 1;
        if (
          text[to] === ":" &&
          DEFINITION_LEAD.test(text.slice(lineStart, at))
        ) {
          const textFrom = text[to + 1] === " " ? to + 2 : to + 1;
          defs.push({ from: at, to: textFrom, key, textFrom });
        } else {
          refs.push({ from: at, to, key });
        }
      }
    }
    at = next;
  }

  // Of two definitions of a label the first is the note.
  const noted = new Map<string, Found>();
  for (const def of defs) {
    if (!noted.has(def.key)) {
      noted.set(def.key, def);
    }
  }

  // A definition's span: its line, the lazy lines under it up to a blank,
  // and what is indented four after a blank, until the next definition.
  const starts = new Set(defs.map((def) => def.from));
  const spanEnd = (def: Found): number => {
    let number = doc.lineAt(def.from).number;
    let end = doc.line(number).to;
    let blank = false;
    for (number += 1; number <= doc.lines; number += 1) {
      const line = doc.line(number);
      if (/^\s*$/.test(line.text)) {
        blank = true;
        continue;
      }
      const lead = /^ {0,3}(?:> ?)* {0,3}/.exec(line.text)?.[0].length ?? 0;
      if (
        starts.has(line.from + lead) ||
        (blank && !/^( {4}|\t)/.test(line.text))
      ) {
        break;
      }
      blank = false;
      end = line.to;
    }
    return end;
  };
  const spans = defs.map((def) => ({ def, end: spanEnd(def) }));
  const insideNote = (pos: number) =>
    spans.find((span) => span.def.from < pos && pos < span.end) ?? null;

  const numbers = new Map<string, number>();
  const counted = new Set<Found>();
  const queue: string[] = [];
  const visit = (ref: Found) => {
    if (!noted.has(ref.key)) {
      return;
    }
    counted.add(ref);
    if (!numbers.has(ref.key)) {
      numbers.set(ref.key, numbers.size + 1);
      queue.push(ref.key);
    }
  };
  for (const ref of refs) {
    if (!insideNote(ref.from)) {
      visit(ref);
    }
  }
  // The notes are rendered in number order, and a reference inside one
  // numbers a note that comes after.
  for (let index = 0; index < queue.length; index += 1) {
    const def = noted.get(queue[index] ?? "");
    const span = spans.find((candidate) => candidate.def === def);
    if (!span) {
      continue;
    }
    for (const ref of refs) {
      if (ref.from > span.def.from && ref.from < span.end) {
        visit(ref);
      }
    }
  }

  return {
    refs: refs
      .filter((ref) => counted.has(ref))
      .map(({ from, to, key }) => ({ from, to, key })),
    notes: [...noted.values()]
      .filter((def) => numbers.has(def.key))
      .map(({ from, key, textFrom }) => ({
        from,
        key,
        textFrom: textFrom ?? from,
      }))
      .sort((a, b) => a.from - b.from),
    numbers,
    parsed: syntaxTree(state).length,
  };
}

/** What in a touched line can change which footnotes there are. */
const TRIGGER = /\[\^|[`~]|<!--|-->|\$|^\s*$|^[ \t>]/;

/** Whether the edit can change a reference, a definition or what is code. */
function affects(tr: {
  startState: EditorState;
  state: EditorState;
  changes: ChangeSet;
}): boolean {
  const before = tr.startState.doc;
  const after = tr.state.doc;
  let hit = false;
  tr.changes.iterChanges((fromA, toA, fromB, _toB, inserted) => {
    if (hit) {
      return;
    }
    const first = before.lineAt(fromA);
    const last = before.lineAt(toA);
    if (first.number !== last.number || inserted.lines > 1) {
      hit = true;
      return;
    }
    const now = after.lineAt(fromB);
    hit = TRIGGER.test(first.text) || TRIGGER.test(now.text);
  });
  return hit;
}

const model = StateField.define<Model>({
  create: (state) => scan(state),
  update(value, tr) {
    if (tr.docChanged) {
      if (affects(tr)) {
        return scan(tr.state);
      }
      const map = (pos: number) => tr.changes.mapPos(pos, 1);
      return {
        ...value,
        refs: value.refs.map((ref) => ({
          ...ref,
          from: map(ref.from),
          to: map(ref.to),
        })),
        notes: value.notes.map((note) => ({
          ...note,
          from: map(note.from),
          textFrom: map(note.textFrom),
        })),
        parsed: syntaxTree(tr.state).length,
      };
    }
    // The parse reaching further without an edit: code may have appeared.
    if (syntaxTree(tr.state).length !== value.parsed) {
      return scan(tr.state);
    }
    return value;
  },
});

/** Moves the caret to the text of the note `key` names, if this view may. */
function jump(view: EditorView, from: number, key: string): void {
  const state = view.state;
  const note = state.field(model).notes.find((entry) => entry.key === key);
  if (!note) {
    return;
  }
  // In a table cell the note is outside the cell: nothing to go to.
  const segment = state.facet(markSegment)?.(state, from) ?? null;
  if (segment && (note.textFrom < segment.from || note.textFrom > segment.to)) {
    return;
  }
  view.dispatch({
    selection: EditorSelection.cursor(note.textFrom),
    scrollIntoView: true,
    userEvent: "select",
  });
  view.focus();
}

class RefWidget extends WidgetType {
  readonly number: number;
  readonly key: string;

  constructor(number: number, key: string) {
    super();
    this.number = number;
    this.key = key;
  }

  eq(other: RefWidget): boolean {
    return other.number === this.number && other.key === this.key;
  }

  toDOM(view: EditorView): HTMLElement {
    const sup = document.createElement("sup");
    sup.className = "cm-footnote-ref";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "cm-footnote-button";
    button.setAttribute("aria-label", `Footnote ${this.number}`);
    button.textContent = String(this.number);
    const go = () => jump(view, view.posAtDOM(sup), this.key);
    button.addEventListener("mousedown", (event) => event.preventDefault());
    button.addEventListener("click", (event) => {
      event.preventDefault();
      go();
    });
    button.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        go();
      }
    });
    sup.append(button);
    return sup;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

class LabelWidget extends WidgetType {
  readonly number: number;

  constructor(number: number) {
    super();
    this.number = number;
  }

  eq(other: LabelWidget): boolean {
    return other.number === this.number;
  }

  toDOM(): HTMLElement {
    const label = document.createElement("span");
    label.className = "cm-footnote-label";
    label.setAttribute("role", "img");
    label.setAttribute("aria-label", `Footnote ${this.number} text`);
    label.textContent = String(this.number);
    return label;
  }
}

function build(view: EditorView): DecorationSet {
  const { state } = view;
  const { refs, notes, numbers } = state.field(model);
  const marks: ReturnType<Decoration["range"]>[] = [];
  const hidden = (pos: number) =>
    marksHiddenOnLine(state, state.doc.lineAt(pos).number);
  for (const { from, to } of view.visibleRanges) {
    for (const ref of refs) {
      if (ref.to >= from && ref.from <= to && hidden(ref.from)) {
        marks.push(
          Decoration.replace({
            widget: new RefWidget(numbers.get(ref.key) ?? 0, ref.key),
          }).range(ref.from, ref.to),
        );
      }
    }
    for (const note of notes) {
      if (note.textFrom >= from && note.from <= to && hidden(note.from)) {
        marks.push(
          Decoration.replace({
            widget: new LabelWidget(numbers.get(note.key) ?? 0),
          }).range(note.from, note.textFrom),
        );
      }
    }
  }
  return Decoration.set(marks, true);
}

const drawing = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = build(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.viewportChanged ||
        (update.selectionSet && update.state.facet(marksOnEditLine)) ||
        update.startState.field(model) !== update.state.field(model) ||
        parseAdvanced(update) ||
        parkingChanged(update) ||
        markModeChanged(update)
      ) {
        this.decorations = build(update.view);
      }
    }
  },
  {
    decorations: (value) => value.decorations,
    // A drawn reference is one place for the caret: arrows step over it and
    // Backspace takes the whole `[^label]` (`marks/deleting.ts` crosses atoms).
    provide: (plugin) =>
      EditorView.atomicRanges.of(
        (view) => view.plugin(plugin)?.decorations ?? Decoration.none,
      ),
  },
);

const theme = EditorView.baseTheme({
  ".cm-footnote-ref": {
    fontSize: "0.75em",
    lineHeight: "0",
    verticalAlign: "super",
  },
  ".cm-footnote-button": {
    all: "unset",
    cursor: "pointer",
    color: "var(--link)",
    padding: "0 0.15em",
    borderRadius: "0.2rem",
    "&:focus-visible": { outline: "2px solid var(--link)" },
  },
  ".cm-footnote-label": {
    fontSize: "0.75em",
    fontWeight: "600",
    color: "var(--color-slate-500)",
    marginRight: "0.4em",
  },
});

/** Footnotes as numbers; belongs in the preview layers of a document kind. */
export function footnotes(): Extension {
  return [model, drawing, theme];
}
