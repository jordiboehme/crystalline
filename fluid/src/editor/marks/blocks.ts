/**
 * Bullets, numbers, task boxes, quotes and rules drawn in place of their
 * marks, and the keys that go with them.
 *
 * On a line whose marks are hidden (`marksHiddenOnLine`) the marks the model
 * finds (`hiddenMarks.ts`) are replaced: a list mark by the bullet or number
 * the reading page shows, a task marker by its box, the `>` of a quote by a
 * bar on the line, and a thematic break by a rule. Every replacement is
 * atomic, so the caret steps over it and Backspace takes it whole. Nothing
 * here rewrites the buffer: the source number of an ordered list is never
 * touched, and the one write is a task box, which is a one character edit.
 *
 * `listEnter` continues a list item or a quote on Enter and `taskKeymap`
 * flips a task with Mod-Enter. Both belong to the layer, so neither runs in
 * Raw.
 */

import { isolateHistory } from "@codemirror/commands";
import { completionStatus } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type {
  ChangeSpec,
  EditorState,
  Extension,
  Range,
  SelectionRange,
} from "@codemirror/state";
import { EditorSelection, Prec } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  keymap,
} from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";

import { frontmatterRegion } from "../frontmatterRegion";
import { parkingChanged } from "../parkedSelection";
import { parseAdvanced } from "../parseProgress";
import { roomUndo } from "../roomUndo";
import type { Mark } from "./hiddenMarks";
import { marksOnLine } from "./hiddenMarks";
import { markModeChanged, marksHiddenOnLine, marksOnEditLine } from "./mode";

/** The three states of a task box. */
export type TaskState = "open" | "done" | "na";

const TASK_CHARACTER: Record<TaskState, string> = {
  open: " ",
  done: "x",
  na: "~",
};

/** The task marker of the line `at` is on, whether its marks show or not. */
function taskMark(state: EditorState, at: number): Mark | null {
  const line = state.doc.lineAt(at);
  return (
    marksOnLine(state, line.number, { always: true }).find(
      (mark) => mark.kind === "task",
    ) ?? null
  );
}

/**
 * The one writer of a task's state: the middle character of the box on the
 * line `at` is on, nothing else. Null when the line holds no task box. The
 * click, Mod-Enter and the task menu all write through it.
 */
export function taskChanges(
  state: EditorState,
  at: number,
  to: TaskState,
): { from: number; to: number; insert: string } | null {
  const mark = taskMark(state, at);
  if (!mark) {
    return null;
  }
  return { from: mark.from + 1, to: mark.from + 2, insert: TASK_CHARACTER[to] };
}

/** The state of the box on the line `at` is on, or null. */
function taskStateAt(state: EditorState, at: number): TaskState | null {
  const mark = taskMark(state, at);
  if (!mark) {
    return null;
  }
  const middle = state.doc.sliceString(mark.from + 1, mark.from + 2);
  if (middle === "~") {
    return "na";
  }
  return middle === "x" || middle === "X" ? "done" : "open";
}

/** Whether the page takes a write: an ended Reflection does not. */
function writable(state: EditorState): boolean {
  return state.facet(EditorView.editable) && !state.readOnly;
}

/** Writes a task state as a gesture of its own: one undo step, solo or in a room. */
function writeTask(view: EditorView, at: number, to: TaskState): boolean {
  const { state } = view;
  if (!writable(state)) {
    return false;
  }
  const changes = taskChanges(state, at, to);
  if (!changes) {
    return false;
  }
  const room = state.facet(roomUndo);
  room?.stopCapturing();
  view.dispatch({
    changes,
    userEvent: "input",
    annotations: isolateHistory.of("full"),
  });
  room?.stopCapturing();
  return true;
}

/** Done becomes open, open becomes done, not applicable becomes open. */
function cycled(state: TaskState): TaskState {
  return state === "open" ? "done" : "open";
}

/** A task box a click or Space flips. */
class CheckboxWidget extends WidgetType {
  readonly checked: boolean;

  constructor(checked: boolean) {
    super();
    this.checked = checked;
  }

  override eq(other: CheckboxWidget): boolean {
    return other.checked === this.checked;
  }

  override toDOM(view: EditorView): HTMLElement {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = this.checked;
    input.className = "cm-task-toggle";
    input.setAttribute(
      "aria-label",
      this.checked ? "Mark task open" : "Mark task done",
    );
    flipOn(input, view, this.checked ? "open" : "done");
    return input;
  }

  override ignoreEvent(): boolean {
    // The widget owns its events; the editor must not also treat the click
    // as a selection change on the hidden marker text.
    return true;
  }
}

/**
 * The not-applicable box `[~]`, drawn as a struck box. A click marks the task
 * open.
 */
class NotApplicableWidget extends WidgetType {
  override eq(): boolean {
    return true;
  }

  override toDOM(view: EditorView): HTMLElement {
    const span = document.createElement("span");
    span.className = "cm-task-na";
    span.setAttribute("role", "checkbox");
    span.setAttribute("aria-checked", "mixed");
    span.setAttribute("aria-label", "Not applicable");
    span.tabIndex = 0;
    flipOn(span, view, "open");
    return span;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

/** The primary press, a bare click and Space or Enter on a focused box write `to` once. */
function flipOn(box: HTMLElement, view: EditorView, to: TaskState): void {
  // A press that already wrote must not write again when the browser then
  // sends its click (after a mouse press, or on the keyup of Space).
  let handled = false;
  const flip = () => {
    writeTask(view, view.posAtDOM(box), to);
  };
  box.addEventListener("mousedown", (event) => {
    // Only the primary button flips; a right click is the task menu's.
    if (event.button !== 0) {
      return;
    }
    // The press must not also move the selection into the hidden marker.
    event.preventDefault();
    handled = true;
    flip();
  });
  box.addEventListener("click", (event) => {
    event.preventDefault();
    if (handled) {
      handled = false;
      return;
    }
    flip();
  });
  box.addEventListener("keydown", (event) => {
    if (event.key === " " || event.key === "Enter") {
      event.preventDefault();
      if (!event.repeat) {
        handled = true;
        flip();
      }
    }
  });
  box.addEventListener("keyup", () => {
    // The click of Space arrives right after the keyup; drop the flag after.
    setTimeout(() => {
      handled = false;
    }, 0);
  });
}

/** A bullet or a number, in the room the marker took. */
class MarkerWidget extends WidgetType {
  readonly text: string;
  readonly className: string;

  constructor(text: string, className: string) {
    super();
    this.text = text;
    this.className = className;
  }

  override eq(other: MarkerWidget): boolean {
    return other.text === this.text && other.className === this.className;
  }

  override toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = this.className;
    span.setAttribute("aria-hidden", "true");
    span.textContent = this.text;
    return span;
  }
}

/** A thematic break drawn as a rule. */
class RuleWidget extends WidgetType {
  override eq(): boolean {
    return true;
  }

  override toDOM(): HTMLElement {
    const rule = document.createElement("span");
    rule.className = "cm-hr";
    rule.setAttribute("role", "separator");
    return rule;
  }
}

const BULLETS = ["•", "◦", "▪"];

/** The list item the list mark at `from` opens, with its list. */
function itemAt(
  state: EditorState,
  from: number,
): { item: SyntaxNode; depth: number } | null {
  let node: SyntaxNode | null = syntaxTree(state).resolveInner(from, 1);
  while (node && node.name !== "ListItem") {
    node = node.parent;
  }
  if (!node) {
    return null;
  }
  let depth = 0;
  for (let up = node.parent; up; up = up.parent) {
    if (up.name === "BulletList" || up.name === "OrderedList") {
      depth += 1;
    }
  }
  return { item: node, depth };
}

/** What a list mark is drawn as: a bullet by depth, or the number the page shows. */
function listWidget(state: EditorState, mark: Mark): WidgetType | null {
  const found = itemAt(state, mark.from);
  if (!found) {
    return null;
  }
  const { item, depth } = found;
  const written = /^(\d+)([.)])/.exec(
    state.doc.sliceString(mark.from, mark.to),
  );
  if (!written) {
    return new MarkerWidget(
      BULLETS[Math.min(depth, BULLETS.length) - 1] ?? BULLETS[0] ?? "•",
      "cm-list-bullet",
    );
  }
  let index = 0;
  let first = item;
  for (let before = item.prevSibling; before; before = before.prevSibling) {
    if (before.name === "ListItem") {
      index += 1;
      first = before;
    }
  }
  const start = /^\d+/.exec(
    state.doc.sliceString(first.from, Math.min(first.to, first.from + 12)),
  );
  const number = Number(start?.[0] ?? written[1]) + index;
  return new MarkerWidget(`${number}${written[2]}`, "cm-list-number");
}

interface Drawn {
  decorations: DecorationSet;
  atoms: DecorationSet;
}

function build(view: EditorView): Drawn {
  const { state } = view;
  const doc = state.doc;
  const all: Range<Decoration>[] = [];
  const atoms: Range<Decoration>[] = [];
  const replace = (from: number, to: number, widget?: WidgetType) => {
    const range = Decoration.replace(widget ? { widget } : {}).range(from, to);
    all.push(range);
    atoms.push(range);
  };
  const frontmatterEnd = frontmatterRegion(doc)?.to ?? -1;
  // A line two visible ranges both reach is drawn once.
  let drawnTo = 0;
  for (const { from, to } of view.visibleRanges) {
    let number = Math.max(doc.lineAt(from).number, drawnTo + 1);
    const last = doc.lineAt(to).number;
    for (; number <= last; number += 1) {
      const marks = marksOnLine(state, number).filter(
        (mark) =>
          mark.kind === "list" || mark.kind === "task" || mark.kind === "quote",
      );
      let depth = 0;
      for (const mark of marks) {
        if (mark.kind === "quote") {
          depth += 1;
          replace(mark.from, mark.to);
        } else if (mark.kind === "list") {
          const boxed = marks.some(
            (other) => other.kind === "task" && other.from === mark.to,
          );
          // A task line has no bullet: the box stands where it was.
          const widget = boxed ? null : listWidget(state, mark);
          replace(mark.from, mark.to, widget ?? undefined);
        } else {
          const box = mark.from + 3;
          const text = doc.sliceString(mark.from, box);
          replace(
            mark.from,
            box,
            text === "[~]"
              ? new NotApplicableWidget()
              : new CheckboxWidget(text.toLowerCase() === "[x]"),
          );
          if (box < mark.to) {
            replace(box, mark.to);
          }
        }
      }
      if (depth > 0) {
        all.push(
          Decoration.line({
            class: `cm-quote-line cm-quote-${Math.min(depth, 3)}`,
          }).range(doc.line(number).from),
        );
      }
    }
    drawnTo = last;
    syntaxTree(state).iterate({
      from,
      to,
      enter: (ref) => {
        if (ref.name !== "HorizontalRule" || ref.from <= frontmatterEnd) {
          return;
        }
        const at = doc.lineAt(ref.from);
        if (ref.to <= at.to && marksHiddenOnLine(state, at.number)) {
          replace(ref.from, ref.to, new RuleWidget());
        }
      },
    });
  }
  return {
    decorations: Decoration.set(all, true),
    atoms: Decoration.set(atoms, true),
  };
}

const blockPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    atoms: DecorationSet;

    constructor(view: EditorView) {
      ({ decorations: this.decorations, atoms: this.atoms } = build(view));
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        // A selection reveals marks only with the setting on.
        (update.selectionSet && update.state.facet(marksOnEditLine)) ||
        update.viewportChanged ||
        parseAdvanced(update) ||
        parkingChanged(update) ||
        markModeChanged(update)
      ) {
        ({ decorations: this.decorations, atoms: this.atoms } = build(
          update.view,
        ));
      }
    }
  },
  {
    decorations: (value) => value.decorations,
    provide: (plugin) =>
      EditorView.atomicRanges.of(
        (view) => view.plugin(plugin)?.atoms ?? Decoration.none,
      ),
  },
);

const blockTheme = EditorView.baseTheme({
  ".cm-task-toggle": { verticalAlign: "middle", margin: "0 0.15em" },
  // A box the size of the checkbox beside it, struck through once.
  ".cm-task-na": {
    display: "inline-block",
    width: "0.8em",
    height: "0.8em",
    margin: "0 0.2em",
    verticalAlign: "-0.05em",
    border: "1px solid var(--color-slate-400)",
    borderRadius: "2px",
    cursor: "pointer",
    background:
      "linear-gradient(var(--color-slate-500), var(--color-slate-500)) center / 100% 1px no-repeat",
  },
  ".cm-list-bullet": {
    display: "inline-block",
    minWidth: "1.25em",
    textAlign: "center",
    color: "var(--color-slate-500)",
  },
  ".cm-list-number": {
    display: "inline-block",
    minWidth: "1.5em",
    paddingRight: "0.3em",
    textAlign: "right",
    color: "var(--color-slate-500)",
    fontVariantNumeric: "tabular-nums",
  },
  // The bars of a quote: one per depth, a callout keeps its own rule.
  ".cm-quote-line:not(.callout)": {
    paddingLeft: "0.9em",
    color: "var(--color-slate-600)",
    boxShadow: "inset 3px 0 0 var(--color-slate-300)",
  },
  ".cm-quote-line.cm-quote-2:not(.callout)": {
    paddingLeft: "1.4em",
    boxShadow:
      "inset 3px 0 0 var(--color-slate-300), inset 8px 0 0 var(--color-slate-200)",
  },
  ".cm-quote-line.cm-quote-3:not(.callout)": {
    paddingLeft: "1.9em",
    boxShadow:
      "inset 3px 0 0 var(--color-slate-300), inset 8px 0 0 var(--color-slate-200), inset 13px 0 0 var(--color-slate-100)",
  },
  ".cm-hr": {
    display: "inline-block",
    width: "100%",
    height: "0",
    verticalAlign: "middle",
    borderTop: "1px solid var(--color-slate-300)",
  },
});

/** The block marks drawn: bullets, numbers, task boxes, quote bars and rules. */
export const blockDrawing: Extension = [blockPlugin, blockTheme];

// -- Enter -----------------------------------------------------------------

/** The next marker text: the number plus one, a fresh open box. */
function nextMarker(text: string, mark: Mark, last: boolean): string {
  if (mark.kind === "task") {
    return text.replace(/\[[xX~]\]/, "[ ]");
  }
  if (mark.kind === "list" && last) {
    return text.replace(/^(\d+)([.)])/, (_whole, digits: string, d: string) => {
      const next = String(Number(digits) + 1);
      return `${digits.startsWith("0") ? next.padStart(digits.length, "0") : next}${d}`;
    });
  }
  return text;
}

interface Continuation {
  /** Where the prefix ends. */
  end: number;
  next: string;
  /** Where a removal of an empty item starts. */
  removeFrom: number;
}

/** The marker chain of the line the caret is on, or null off a list or quote. */
function continuation(
  state: EditorState,
  range: SelectionRange,
): Continuation | null {
  if (!range.empty) {
    return null;
  }
  const line = state.doc.lineAt(range.head);
  const chain: Mark[] = [];
  let reach = line.from;
  for (const mark of marksOnLine(state, line.number, { always: true })) {
    if (mark.kind !== "list" && mark.kind !== "task" && mark.kind !== "quote") {
      continue;
    }
    if (state.doc.sliceString(reach, mark.from).trim() !== "") {
      break;
    }
    chain.push(mark);
    reach = mark.to;
  }
  const tail = chain.at(-1);
  if (!tail || range.head < tail.to) {
    return null;
  }
  const lists = chain.some((mark) => mark.kind !== "quote");
  if (lists) {
    // The opening line of a fenced block in an item is code, not a new item.
    for (
      let node: SyntaxNode | null = syntaxTree(state).resolveInner(
        range.head,
        -1,
      );
      node;
      node = node.parent
    ) {
      if (node.name === "FencedCode" || node.name === "CodeBlock") {
        return null;
      }
    }
  }
  let next = "";
  let at = line.from;
  const lastList = chain.findLastIndex((mark) => mark.kind === "list");
  chain.forEach((mark, index) => {
    next += state.doc.sliceString(at, mark.from);
    next += nextMarker(
      state.doc.sliceString(mark.from, mark.to),
      mark,
      index === lastList,
    );
    at = mark.to;
  });
  const lastQuote = chain.findLast((mark) => mark.kind === "quote");
  return {
    end: tail.to,
    next,
    removeFrom: lists && lastQuote ? lastQuote.to : line.from,
  };
}

/**
 * Enter in a list item or a quote line: the line break, the indentation and
 * the next marker, and nothing else (the items below are never renumbered).
 * An item holding only its marker loses the marker instead. Anywhere else it
 * is not handled and CodeMirror's Enter runs.
 */
function listEnterCommand(view: EditorView): boolean {
  const { state } = view;
  if (state.readOnly || completionStatus(state) === "active") {
    return false;
  }
  const found = state.selection.ranges.map((range) =>
    continuation(state, range),
  );
  if (found.some((entry) => entry === null)) {
    return false;
  }
  view.dispatch(
    state.update(
      state.changeByRange((range) => {
        const at = state.selection.ranges.indexOf(range);
        const entry = found[at];
        if (!entry) {
          return { range };
        }
        const line = state.doc.lineAt(range.head);
        if (state.doc.sliceString(entry.end, line.to).trim() === "") {
          const changes: ChangeSpec = {
            from: entry.removeFrom,
            to: line.to,
          };
          return {
            changes,
            range: EditorSelection.cursor(entry.removeFrom),
          };
        }
        const insert = state.lineBreak + entry.next;
        return {
          changes: { from: range.head, insert },
          range: EditorSelection.cursor(range.head + insert.length),
        };
      }),
      { userEvent: "input", scrollIntoView: true },
    ),
  );
  return true;
}

/** Mod-Enter on a task line: open to done to open, not applicable to open. */
function cycleTask(view: EditorView): boolean {
  const { state } = view;
  const at = state.selection.main.head;
  const current = taskStateAt(state, at);
  if (current === null) {
    return false;
  }
  // On a page that takes no write the key is swallowed, not left to the
  // default Mod-Enter, which would add a blank line.
  writeTask(view, at, cycled(current));
  return true;
}

/** Enter continues a list or a quote; the highest precedence, never in Raw. */
export const listEnter: Extension = Prec.highest(
  keymap.of([{ key: "Enter", run: listEnterCommand }]),
);

/** Mod-Enter flips the task on the caret line. */
export const taskKeymap: Extension = Prec.highest(
  keymap.of([{ key: "Mod-Enter", run: cycleTask }]),
);
