/**
 * The code box: a closed fence drawn as a box with a header where its fence
 * lines are. Wherever the fence's lines hide their marks (every line by
 * default; with the setting on, until a selection touches a fence line or
 * the first or last code line), the opening fence line becomes the header
 * (a button with the language's name, which opens the "Code language" menu,
 * and "More code block actions", which opens "Code block actions") and the
 * closing fence line folds away. The code lines between wear the box. Where
 * the fences show, they show as written, inside the same box.
 *
 * Which fences draw is `codeFenceOf`'s rule (`elementVerbs.ts`): closed, with
 * a code line, outside the frontmatter, a quote and an id line, and not a
 * mermaid fence (Part 4) or a math fence (`elementPreviews.ts`). An unclosed
 * fence keeps today's look.
 *
 * Nothing here writes: the box is a view over the source, a selection over it
 * copies the source, and the caret never rests on a hidden fence line. The
 * arrows step over one (`fenceArrows`, `caretOffFenceLines`), and Backspace and
 * Delete never join one to its neighbour (`deleting.ts`).
 *
 * A `StateField`, because CodeMirror takes block decorations only from one.
 * It reads only the block level of the syntax tree, and keeps (maps) what it
 * drew while an edit changes no box's lines, word or face.
 */

import { syntaxTree } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { isolateHistory } from "@codemirror/commands";
import type { Annotation, Extension, Range, Text } from "@codemirror/state";
import {
  EditorSelection,
  EditorState,
  Prec,
  StateField,
  Transaction,
} from "@codemirror/state";
import type { Command, DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  keymap,
} from "@codemirror/view";

import type { ElementEdit } from "./elementEdit";
import {
  answersElementEdits,
  macMenuClick,
  requestElementEdit,
  writable,
} from "./elementEdit";
import type { CodeFence } from "./elementVerbs";
import { codeFenceAt, codeFenceOf, languageOf } from "./elementVerbs";
import { composing } from "./marks/caret";
import {
  hiddenMarkLayerOn,
  markModeChanged,
  marksHiddenOnLine,
  marksOnEditLine,
} from "./marks/mode";
import { mainSelectionParked, parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";

/** The name the header shows for an info word. */
export function languageName(word: string | null): string {
  // A math fence the formula layer does not draw (an empty one) is still a
  // formula, never "Plain text".
  if (word === "math") {
    return "Math";
  }
  return languageOf(word)?.name ?? "Plain text";
}

/** One entry of the "Code language" menu, and the word it writes. */
export interface LanguageChoice {
  name: string;
  /** The info word, or null for "Plain text" (no word). */
  word: string | null;
}

/**
 * Every language the editor knows, after "Plain text". Each writes its first
 * alias (`alias[0]`, the name in lower case when it has none). An entry
 * whose word could not be an info word (it holds a space) or would name
 * another language when read back is left out.
 */
export const LANGUAGE_CHOICES: readonly LanguageChoice[] = [
  { name: "Plain text", word: null },
  ...languages
    .map((language) => ({
      name: language.name,
      word: language.alias[0] ?? language.name.toLowerCase(),
    }))
    .filter(
      ({ name, word }) =>
        !/[\s`]/.test(word) && languageOf(word)?.name === name,
    ),
];

/** A drawn box: its fence and the name in its header. */
interface Box {
  fence: CodeFence;
  name: string;
}

interface CodeBoxes {
  boxes: Box[];
  /** Which boxes show their fences, as one comparable word. */
  shown: string;
  decorations: DecorationSet;
  /** The hidden fence lines' ranges: what the caret steps over whole. */
  hidden: DecorationSet;
  /** The numbers of the hidden fence lines. */
  hiddenLines: ReadonlySet<number>;
}

/** The block nodes a fence can sit in that the box draws. */
const CONTAINERS = new Set([
  "Document",
  "BulletList",
  "OrderedList",
  "ListItem",
]);

/** Every code box of the text: a walk of the tree's block level only. */
function scan(state: EditorState): Box[] {
  const boxes: Box[] = [];
  syntaxTree(state).iterate({
    enter: (node) => {
      if (node.name === "FencedCode") {
        const fence = codeFenceOf(state, node.node);
        if (fence !== null) {
          const word = state.doc.sliceString(fence.word.from, fence.word.to);
          boxes.push({ fence, name: languageName(word) });
        }
        return false;
      }
      return CONTAINERS.has(node.name) ? undefined : false;
    },
  });
  return boxes;
}

/**
 * Whether a box shows its fence lines: one of them, or the first or last
 * code line, shows its marks.
 */
function showsFences(state: EditorState, fence: CodeFence): boolean {
  return [fence.open, fence.open + 1, fence.close - 1, fence.close].some(
    (number) => !marksHiddenOnLine(state, number),
  );
}

function shownKey(state: EditorState, boxes: Box[]): string {
  return boxes
    .map((box, index) => (showsFences(state, box.fence) ? index : -1))
    .filter((index) => index !== -1)
    .join(",");
}

/** What a box's drawing depends on, so an edit that keeps it can map it. */
function layoutKey(state: EditorState, boxes: Box[], shown: string): string {
  return `${shown}|${boxes
    .map(
      ({ fence, name }) =>
        `${String(fence.open)}:${String(fence.close)}:${name}:${state.doc.sliceString(fence.word.from, fence.word.to)}`,
    )
    .join(";")}`;
}

/**
 * Where a press on a header button asks for its menu: on the box the header
 * stands for, read at press time. Only the primary button, never a Mac
 * Ctrl+click (the browser's menu), only where marks are hidden and the page
 * takes writes. The press never places a caret.
 */
function ask(
  view: EditorView,
  dom: HTMLElement,
  button: HTMLElement,
  kind: "language" | "code-actions",
): void {
  const { state } = view;
  if (
    !state.facet(hiddenMarkLayerOn) ||
    !writable(state) ||
    !answersElementEdits(view)
  ) {
    return;
  }
  const fence = codeFenceAt(state, view.posAtDOM(dom));
  if (fence === null) {
    return;
  }
  const edit: ElementEdit = {
    kind,
    at: fence.from,
    rect: button.getBoundingClientRect(),
  };
  requestElementEdit(view, edit);
}

/**
 * The header button's ways in: the primary press (which places no caret),
 * and a bare click (a screen reader's activation, Enter or Space on the
 * focused button). A press asks once: the click the browser sends after it
 * asks nothing more.
 */
function askOn(
  view: EditorView,
  dom: HTMLElement,
  button: HTMLElement,
  kind: "language" | "code-actions",
): void {
  let pressed = false;
  button.addEventListener("mousedown", (event) => {
    if (event.button !== 0 || macMenuClick(event)) {
      return;
    }
    event.preventDefault();
    pressed = true;
    ask(view, dom, button, kind);
  });
  button.addEventListener("click", (event) => {
    event.preventDefault();
    if (macMenuClick(event)) {
      return;
    }
    if (pressed) {
      pressed = false;
      return;
    }
    ask(view, dom, button, kind);
  });
}

const SVG = "http://www.w3.org/2000/svg";

/** Three dots in a row, the "more actions" sign. */
function moreIcon(): SVGSVGElement {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", "16");
  svg.setAttribute("height", "16");
  svg.setAttribute("fill", "currentColor");
  svg.setAttribute("aria-hidden", "true");
  for (const cx of ["5", "12", "19"]) {
    const dot = document.createElementNS(SVG, "circle");
    dot.setAttribute("cx", cx);
    dot.setAttribute("cy", "12");
    dot.setAttribute("r", "1.75");
    svg.appendChild(dot);
  }
  return svg;
}

/** The box's header, in place of its opening fence line. */
class CodeHeaderWidget extends WidgetType {
  readonly name: string;

  constructor(name: string) {
    super();
    this.name = name;
  }

  override eq(other: CodeHeaderWidget): boolean {
    return other.name === this.name;
  }

  override toDOM(view: EditorView): HTMLElement {
    const dom = document.createElement("div");
    dom.className = "cm-code-header";
    const language = document.createElement("button");
    language.type = "button";
    language.className = "cm-code-language";
    language.setAttribute("aria-label", `Code language: ${this.name}`);
    language.textContent = this.name;
    askOn(view, dom, language, "language");
    const more = document.createElement("button");
    more.type = "button";
    more.className = "cm-code-more";
    more.setAttribute("aria-label", "More code block actions");
    more.title = "More code block actions";
    more.appendChild(moreIcon());
    askOn(view, dom, more, "code-actions");
    dom.append(language, more);
    return dom;
  }

  /** The header is not text: CodeMirror places no caret for a press on it. */
  override ignoreEvent(): boolean {
    return true;
  }
}

const HIDDEN_FENCE = Decoration.replace({ block: true });

function lineClass(classes: string): Decoration {
  return Decoration.line({ class: classes });
}

const CODE_LINE = lineClass("cm-code-box");
const LAST_CODE_LINE = lineClass("cm-code-box cm-code-box-last");

function draw(state: EditorState, boxes: Box[], shown: string): CodeBoxes {
  const doc = state.doc;
  const showing = new Set(shown.split(","));
  const out: Range<Decoration>[] = [];
  const hidden: Range<Decoration>[] = [];
  const hiddenLines = new Set<number>();
  boxes.forEach(({ fence, name }, index) => {
    const open = doc.line(fence.open);
    const close = doc.line(fence.close);
    if (showing.has(String(index))) {
      for (let number = fence.open; number <= fence.close; number += 1) {
        const edge =
          number === fence.open
            ? " cm-code-fence cm-code-box-first"
            : number === fence.close
              ? " cm-code-fence cm-code-box-last"
              : "";
        out.push(lineClass(`cm-code-box${edge}`).range(doc.line(number).from));
      }
      return;
    }
    const head = Decoration.replace({
      widget: new CodeHeaderWidget(name),
      block: true,
    }).range(open.from, open.to);
    const foot = HIDDEN_FENCE.range(close.from, close.to);
    out.push(head, foot);
    hidden.push(head, foot);
    hiddenLines.add(fence.open);
    hiddenLines.add(fence.close);
    for (let number = fence.open + 1; number < fence.close; number += 1) {
      out.push(
        (number === fence.close - 1 ? LAST_CODE_LINE : CODE_LINE).range(
          doc.line(number).from,
        ),
      );
    }
  });
  return {
    boxes,
    shown,
    decorations: Decoration.set(out, true),
    hidden: Decoration.set(hidden, true),
    hiddenLines,
  };
}

function build(state: EditorState): CodeBoxes {
  const boxes = scan(state);
  return draw(state, boxes, shownKey(state, boxes));
}

const codeBoxField = StateField.define<CodeBoxes>({
  create: build,
  update: (value, tr) => {
    if (tr.docChanged || parseAdvanced(tr)) {
      const boxes = scan(tr.state);
      const shown = shownKey(tr.state, boxes);
      // An edit inside a code line keeps every box as it was: what was
      // drawn moves with the text instead of being drawn again.
      if (
        !parseAdvanced(tr) &&
        layoutKey(tr.state, boxes, shown) ===
          layoutKey(tr.startState, value.boxes, value.shown)
      ) {
        return {
          boxes,
          shown,
          decorations: value.decorations.map(tr.changes),
          hidden: value.hidden.map(tr.changes),
          hiddenLines: value.hiddenLines,
        };
      }
      return draw(tr.state, boxes, shown);
    }
    // A caret move only matters where the setting shows marks.
    const asks =
      (tr.selection !== undefined && tr.state.facet(marksOnEditLine)) ||
      parkingChanged(tr) ||
      markModeChanged(tr);
    if (!asks) {
      return value;
    }
    const shown = shownKey(tr.state, value.boxes);
    return shown === value.shown ? value : draw(tr.state, value.boxes, shown);
  },
  provide: (field) => [
    EditorView.decorations.from(field, (value) => value.decorations),
    // A hidden fence line is not a place a caret steps into.
    EditorView.atomicRanges.of((view) => view.state.field(field).hidden),
  ],
});

/**
 * Whether line `number` (1-based) is a fence line the code box hides. False
 * without the box (Raw, a table cell).
 */
export function hiddenFenceLine(state: EditorState, number: number): boolean {
  return state.field(codeBoxField, false)?.hiddenLines.has(number) ?? false;
}

/** The drawn box whose hidden fence line is line `number`, or null. */
function boxOnFence(state: EditorState, number: number): CodeFence | null {
  const value = state.field(codeBoxField, false);
  if (!value?.hiddenLines.has(number)) {
    return null;
  }
  return (
    value.boxes.find(
      ({ fence }) => fence.open === number || fence.close === number,
    )?.fence ?? null
  );
}

/**
 * Where a caret on a hidden fence line goes, by where it came from
 * (`before`, null when it came from no step), or null when `head` is on no
 * hidden fence line. Two boxes can touch, so it goes on until it rests on a
 * line that is not a hidden fence.
 */
function offFence(
  state: EditorState,
  head: number,
  before: number | null,
): number | null {
  let at = head;
  // Each pass leaves one fence line; a few boxes touch at most.
  for (let pass = 0; pass < 8; pass += 1) {
    const next = offOneFence(state, at, before);
    if (next === null || next === at) {
      return pass === 0 ? next : at;
    }
    at = next;
  }
  return at;
}

function offOneFence(
  state: EditorState,
  head: number,
  before: number | null,
): number | null {
  const doc = state.doc;
  const line = doc.lineAt(head);
  const fence = boxOnFence(state, line.number);
  if (fence === null) {
    return null;
  }
  if (line.number === fence.open) {
    const fromBelow = before !== null && before > line.to;
    if (fromBelow && fence.open > 1) {
      return doc.line(fence.open - 1).to;
    }
    return doc.line(fence.open + 1).from;
  }
  const fromAbove = before !== null && before < line.from;
  // With no line below (the box ends the document: Mod-End, a press below
  // the text), the edge of the last code line is the place to rest.
  if (fromAbove && fence.close < doc.lines) {
    return doc.line(fence.close + 1).from;
  }
  return doc.line(fence.close - 1).to;
}

/**
 * A caret never rests on a hidden fence line: both its ends are positions,
 * and typing there would write into the fence. A step of this person's
 * (`select`) goes on the way it came: down into the box from above, out of
 * it to the line below, up out of it to the line above, and where there is
 * no such line (a box that starts or ends the document: an arrow, Mod-Home,
 * Mod-End, a press below the text) it rests on the box's first or last code
 * line.
 * Any other caret (a commit, a mapped caret) goes into the box. Default
 * precedence, and never while main's selection is parked behind a cell or an
 * IME composes. A remote change sets no selection, so it is never steered:
 * a caret it leaves on a hidden fence line is `writesOffFenceLines`' to keep
 * out of the fence.
 */
const caretOffFenceLines = EditorState.transactionFilter.of((tr) => {
  if (
    tr.selection === undefined ||
    tr.isUserEvent("input.type.compose") ||
    tr.startState.facet(mainSelectionParked) ||
    composing(tr.startState)
  ) {
    return tr;
  }
  const state = tr.state;
  if (
    state.facet(mainSelectionParked) ||
    (state.field(codeBoxField, false)?.hiddenLines.size ?? 0) === 0
  ) {
    return tr;
  }
  const stepped = tr.isUserEvent("select");
  const selection = state.selection;
  const single = selection.ranges.length === 1;
  const before =
    stepped && single
      ? tr.changes.mapPos(tr.startState.selection.main.head)
      : null;
  let moved = false;
  const ranges = selection.ranges.map((range) => {
    if (!range.empty) {
      return range;
    }
    const pos = offFence(state, range.head, before);
    if (pos === null || pos === range.head) {
      return range;
    }
    moved = true;
    return EditorSelection.cursor(pos);
  });
  if (!moved) {
    return tr;
  }
  return [
    tr,
    {
      selection: EditorSelection.create(ranges, selection.mainIndex),
      sequential: true,
    },
  ];
});

/**
 * After a change that set no selection (a peer's edit, a room undo), or a
 * parse that moved on and hid the line (D64), left the caret on a hidden
 * fence line, a follow-up moves it into the box: a
 * selection only, kept out of the undo history, so any write of the
 * person's (typing, a toolbar or slash verb, an insert) starts inside the
 * box. The remote transaction itself is never changed.
 */
const caretIntoBoxAfterRemote = ViewPlugin.define((view) => ({
  update(update: ViewUpdate) {
    // A parse that moved on can hide the fence line a caret already rests
    // on: a commit whose own parse stopped short of the box set the caret
    // there while the line was not hidden yet (D64).
    if (
      !(update.docChanged || parseAdvanced(update)) ||
      update.transactions.some((tr) => tr.selection !== undefined)
    ) {
      return;
    }
    const stray = () => {
      const { state } = view;
      const { main } = state.selection;
      if (
        view.composing ||
        state.facet(mainSelectionParked) ||
        state.selection.ranges.length !== 1 ||
        !main.empty
      ) {
        return null;
      }
      return offFence(state, main.head, null);
    };
    if (stray() === null) {
      return;
    }
    // Not from inside an update: once it is over.
    queueMicrotask(() => {
      if ((view as unknown as { destroyed?: boolean }).destroyed === true) {
        return;
      }
      const pos = stray();
      if (pos === null || pos === view.state.selection.main.head) {
        return;
      }
      view.dispatch({
        selection: EditorSelection.cursor(pos),
        annotations: Transaction.addToHistory.of(false),
      });
    });
  },
}));

/** What this person types or pastes, and their deletes. */
const PERSON_WRITES = [
  "input.type",
  "input.paste",
  "input.drop",
  "input.complete",
  "delete",
];

/** The annotations a rebuilt edit keeps from the one it replaces. */
function carried(tr: Transaction): Annotation<unknown>[] {
  const out: Annotation<unknown>[] = [];
  const event = tr.annotation(Transaction.userEvent);
  if (event !== undefined) {
    out.push(Transaction.userEvent.of(event));
  }
  const history = tr.annotation(Transaction.addToHistory);
  if (history !== undefined) {
    out.push(Transaction.addToHistory.of(history));
  }
  const isolate = tr.annotation(isolateHistory);
  if (isolate !== undefined) {
    out.push(isolateHistory.of(isolate));
  }
  return out;
}

/**
 * A fence line the box hides stays byte for byte what it is under this
 * person's typing and deletes. Only a change that sets no selection (a peer's
 * edit, a room undo) can leave the caret on one, and that caret is not
 * steered. The next keystroke from it is: typed or pasted text goes beside
 * the fence (into the first code line from the opening fence, onto the line
 * below the closing one, after a new line break when the box ends the
 * document), and a delete writes nothing and puts the caret into the box.
 */
const writesOffFenceLines = EditorState.transactionFilter.of((tr) => {
  if (
    !tr.docChanged ||
    !PERSON_WRITES.some((event) => tr.isUserEvent(event)) ||
    tr.annotation(Transaction.remote) !== undefined ||
    // A picture's removal names its picture, not the caret: it is no key
    // pressed from a fence line (Part 3, P7).
    tr.isUserEvent("delete.picture")
  ) {
    return tr;
  }
  const start = tr.startState;
  const { selection } = start;
  if (selection.ranges.length !== 1 || !selection.main.empty) {
    return tr;
  }
  const head = selection.main.head;
  const doc = start.doc;
  const line = doc.lineAt(head);
  const fence = boxOnFence(start, line.number);
  if (fence === null) {
    return tr;
  }
  if (tr.isUserEvent("delete")) {
    const inside =
      line.number === fence.open
        ? doc.line(fence.open + 1).from
        : doc.line(fence.close - 1).to;
    return { selection: EditorSelection.cursor(inside), scrollIntoView: true };
  }
  // Only a plain insertion at the caret: anything else is left as it is.
  const inserts: Text[] = [];
  let plain = true;
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    if (fromA !== head || toA !== head) {
      plain = false;
    }
    inserts.push(inserted);
  });
  const typed = inserts[0];
  if (!plain || inserts.length !== 1 || typed === undefined) {
    return tr;
  }
  // The caret the change asked for, as an offset into the text put in.
  const after = tr.newSelection.main.head - head;
  let at: number;
  let text: Text | string = typed;
  let lead = 0;
  if (line.number === fence.open) {
    at = doc.line(fence.open + 1).from;
  } else if (fence.close < doc.lines) {
    at = doc.line(fence.close + 1).from;
  } else {
    at = line.to;
    text = `${start.lineBreak}${typed.toString()}`;
    lead = 1;
  }
  return {
    changes: { from: at, insert: text },
    selection: EditorSelection.cursor(at + lead + after),
    effects: tr.effects,
    scrollIntoView: true,
    annotations: carried(tr),
  };
});

/**
 * ArrowUp and ArrowDown beside a box whose fences are hidden: from the line
 * above into the first code line, from the last code line to the line below
 * (or nowhere, when the box ends the document: "Add a paragraph below" is
 * the way out), and the same upward. A wrapped line first moves within
 * itself, as always.
 */
function fenceArrow(forward: boolean): Command {
  return (view) => {
    const { state } = view;
    const { selection } = state;
    if (selection.ranges.length > 1 || !selection.main.empty) {
      return false;
    }
    const value = state.field(codeBoxField, false);
    if (!value || value.hiddenLines.size === 0) {
      return false;
    }
    const doc = state.doc;
    const range = selection.main;
    const line = doc.lineAt(range.head);
    const next = line.number + (forward ? 1 : -1);
    const fence = boxOnFence(state, next);
    if (fence === null) {
      return false;
    }
    // Over the fence line, into the box or out of it.
    const target =
      next === (forward ? fence.open : fence.close)
        ? next + (forward ? 1 : -1)
        : forward
          ? fence.close < doc.lines
            ? fence.close + 1
            : null
          : fence.open > 1
            ? fence.open - 1
            : null;
    const moved = view.moveVertically(range, forward);
    if (
      moved.head !== range.head &&
      doc.lineAt(moved.head).number === line.number
    ) {
      return false;
    }
    if (target === null) {
      // The box ends (or starts) the document: the caret stays.
      return true;
    }
    // The same column on the line it goes to, or that line's end.
    const to = doc.line(target);
    const head = to.from + Math.min(range.head - line.from, to.length);
    // The goal column goes along, so a move on from the far side keeps the
    // column CodeMirror's own vertical moves keep.
    view.dispatch({
      // A set, not a bare range: a bare range loses its goal column.
      selection: EditorSelection.create([
        EditorSelection.cursor(
          head,
          0,
          undefined,
          moved.goalColumn ?? range.goalColumn,
        ),
      ]),
      userEvent: "select",
      scrollIntoView: true,
    });
    return true;
  };
}

const fenceArrows: Extension = Prec.high(
  keymap.of([
    { key: "ArrowDown", run: fenceArrow(true) },
    { key: "ArrowUp", run: fenceArrow(false) },
  ]),
);

const codeBoxTheme = EditorView.baseTheme({
  ".cm-code-header": {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "0.5rem",
    padding: "0.125rem 0.375rem 0.125rem 0.75rem",
    backgroundColor: "var(--color-slate-100)",
    borderTopLeftRadius: "0.375rem",
    borderTopRightRadius: "0.375rem",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
    color: "var(--color-slate-600)",
  },
  ".cm-code-header button": {
    border: "none",
    background: "none",
    padding: "0.125rem 0.25rem",
    borderRadius: "0.25rem",
    color: "inherit",
    font: "inherit",
    cursor: "pointer",
  },
  ".cm-code-header button:hover": {
    backgroundColor: "var(--color-slate-200)",
  },
  ".cm-code-more": { display: "inline-flex", alignItems: "center" },
  ".cm-line.cm-code-box": {
    backgroundColor: "var(--color-slate-50)",
    paddingLeft: "0.75rem",
    paddingRight: "0.75rem",
  },
  ".cm-line.cm-code-box-first": {
    borderTopLeftRadius: "0.375rem",
    borderTopRightRadius: "0.375rem",
  },
  ".cm-line.cm-code-box-last": {
    borderBottomLeftRadius: "0.375rem",
    borderBottomRightRadius: "0.375rem",
  },
  ".cm-line.cm-code-fence": { color: "var(--color-slate-500)" },
  "&dark .cm-code-header": {
    backgroundColor: "var(--color-slate-800)",
    color: "var(--color-slate-300)",
  },
  "&dark .cm-code-header button:hover": {
    backgroundColor: "var(--color-slate-700)",
  },
  "&dark .cm-line.cm-code-box": {
    backgroundColor: "var(--color-slate-900)",
  },
  "&dark .cm-line.cm-code-fence": { color: "var(--color-slate-400)" },
});

/** The whole layer, inside the preview compartment. */
export function codeBlocks(): Extension {
  return [
    codeBoxField,
    caretOffFenceLines,
    // Listed after the caret rule, so it runs first (filters run last
    // listed first): a write it moves out of a fence sets a caret the caret
    // rule then finds already off the fence.
    writesOffFenceLines,
    caretIntoBoxAfterRemote,
    fenceArrows,
    codeBoxTheme,
  ];
}
