/**
 * Footnotes drawn as numbers. A reference `[^label]` becomes a small raised
 * button showing its number, a definition line `[^label]: text` shows the
 * number in place of `[^label]:` and its space, and a click or Enter on a
 * reference moves the caret to the definition's text. The text stays plain
 * Markdown: every widget is a view over bytes that never change, and a copy
 * over a drawn reference gives the source.
 *
 * What is a reference, a definition or code, and the numbers, come from the
 * reading page's own parser (`footnoteSyntax.ts`), so the two cannot differ
 * by construction. That parse costs far more than a keystroke may (tens of
 * milliseconds for a long page), so the model is a state field that is read
 * again only for an edit that can change footnotes, at once for a short
 * document and, for a long one, in a worker after a short pause (the first
 * read at mount too, with references shown as text until it arrives); until
 * a read arrives the drawn references and notes stay where the edits carry
 * them.
 */

import type { EditorState, Extension } from "@codemirror/state";
import {
  ChangeSet,
  EditorSelection,
  Facet,
  StateEffect,
  StateField,
} from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";

import type { Notes } from "./footnoteSyntax";
import { readNotes } from "./footnoteSyntax";
import { markSegment } from "./marks/caret";
import {
  markModeChanged,
  marksHiddenOnLine,
  marksOnEditLine,
} from "./marks/mode";
import { parkingChanged } from "./parkedSelection";

let scans = 0;

/** How many whole-document reads have run, for the cost tests. */
export function footnoteScans(): number {
  return scans;
}

function read(doc: EditorState["doc"]): Notes {
  scans += 1;
  return readNotes(doc);
}

/** Documents up to this many characters are read inside the keystroke. */
const SYNC_LIMIT = 6000;

/** How long a long document waits after the last edit before it is read. */
const PAUSE_MS = 120;

/** What a footnote read is handed to, in or out of the page's thread. */
export interface NotesWorker {
  postMessage(message: { id: number; text: string }): void;
  terminate(): void;
  onmessage: ((event: { data: { id: number; notes: Notes } }) => void) | null;
  onerror: ((event: unknown) => void) | null;
}

export interface FootnoteOptions {
  /**
   * Starts the worker a long document is read in, or answers null where there
   * is none (jsdom, an old browser): the read then runs in the page's thread
   * after a tick. Called at most once per view, on its first long read.
   */
  worker?: () => NotesWorker | null;
}

function startWorker(): NotesWorker | null {
  if (typeof Worker === "undefined") {
    return null;
  }
  return new Worker(new URL("./footnoteWorker.ts", import.meta.url), {
    type: "module",
  }) as unknown as NotesWorker;
}

const workerFactory = Facet.define<
  () => NotesWorker | null,
  () => NotesWorker | null
>({ combine: (values) => values[0] ?? startWorker });

interface Model extends Notes {
  /** Whether an edit that can change footnotes is not read yet. */
  stale: boolean;
  /** Counts the edits that can change footnotes, to tell a reply is current. */
  revision: number;
}

const settled = StateEffect.define<{ revision: number; notes: Notes }>();

const TRIGGER = /[[\]^`~<>$\\:|]/;
const BLOCKISH = /^[ \t]*([#>*+=_-]|\d+[.)])/;
const BLANK = /^\s*$/;

/**
 * Whether the edit can change a reference, a definition, or what is code, a
 * link address, math or HTML around them. When in doubt it says yes: a line
 * that holds a bracket, a caret, a backtick, a tilde, an angle, a dollar or a
 * backslash is read again whatever the edit was, and so is a change in
 * the indentation or the first characters of a line, a blank line made or
 * lost and a change over a line break.
 */
export function affects(tr: {
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
    const indent = /^[ \t]*/.exec(first.text)?.[0].length ?? 0;
    hit =
      fromA - first.from <= indent ||
      TRIGGER.test(first.text) ||
      TRIGGER.test(now.text) ||
      BLOCKISH.test(first.text) ||
      BLOCKISH.test(now.text) ||
      BLANK.test(first.text) ||
      BLANK.test(now.text);
  });
  return hit;
}

function carry<T extends Notes>(value: T, changes: ChangeSet): T {
  const map = (pos: number) => changes.mapPos(pos, 1);
  return {
    ...value,
    refs: value.refs
      .map((ref) => ({ ...ref, from: map(ref.from), to: map(ref.to) }))
      .filter((ref) => ref.from < ref.to),
    notes: value.notes
      .map((note) => ({
        ...note,
        from: map(note.from),
        textFrom: map(note.textFrom),
      }))
      .filter((note) => note.from < note.textFrom),
  };
}

const EMPTY: Notes = { refs: [], notes: [], numbers: new Map() };

const model = StateField.define<Model>({
  create: (state) =>
    state.doc.length <= SYNC_LIMIT
      ? { ...read(state.doc), stale: false, revision: 0 }
      : // A long document is read off the keystroke, mount included; until
        // the answer comes its references and notes show as text.
        { ...EMPTY, stale: true, revision: 0 },
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(settled) && effect.value.revision === value.revision) {
        return {
          ...effect.value.notes,
          stale: false,
          revision: value.revision,
        };
      }
    }
    if (!tr.docChanged) {
      return value;
    }
    if (!affects(tr)) {
      return carry(value, tr.changes);
    }
    const revision = value.revision + 1;
    if (tr.state.doc.length <= SYNC_LIMIT) {
      return { ...read(tr.state.doc), stale: false, revision };
    }
    return { ...carry(value, tr.changes), stale: true, revision };
  },
});

/** The references and notes a state draws, with their numbers (for tests). */
export function footnoteModel(state: EditorState): {
  refs: { label: string; number: number }[];
  notes: { number: number; text: string }[];
} {
  const { refs, notes, numbers } = state.field(model);
  return {
    refs: refs.map((ref) => ({
      label: state.sliceDoc(ref.from, ref.to),
      number: numbers.get(ref.key) ?? 0,
    })),
    notes: notes.map((note) => ({
      number: numbers.get(note.key) ?? 0,
      text: state.sliceDoc(note.textFrom, state.doc.lineAt(note.textFrom).to),
    })),
  };
}

interface Flight {
  id: number;
  revision: number;
  /** Everything that changed the document since the text was handed over. */
  since: ChangeSet;
}

/**
 * Reads a long document off the page's thread: in a worker where there is
 * one, else in a later tick. One read is in flight per view. A reply is
 * applied only if no edit that can change footnotes came after the text it
 * read; edits that cannot are mapped over its offsets; otherwise it is
 * dropped and the pause starts again.
 */
const settling = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | undefined;
    worker: NotesWorker | null = null;
    broken = false;
    destroyed = false;
    flight: Flight | null = null;
    counter = 0;
    readonly view: EditorView;

    constructor(view: EditorView) {
      this.view = view;
      if (view.state.field(model).stale) {
        this.request();
      }
    }

    update(update: ViewUpdate) {
      if (this.flight && update.docChanged) {
        this.flight.since = this.flight.since.compose(update.changes);
      }
      // Only an edit puts the read off; a caret move must not.
      if (
        update.docChanged &&
        update.state.field(model).stale &&
        !this.flight
      ) {
        this.later();
      }
    }

    later() {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.request(), PAUSE_MS);
    }

    request() {
      if (this.destroyed || this.flight) {
        return;
      }
      const { doc } = this.view.state;
      const flight: Flight = {
        id: (this.counter += 1),
        revision: this.view.state.field(model).revision,
        since: ChangeSet.empty(doc.length),
      };
      this.flight = flight;
      if (!this.broken && !this.worker) {
        try {
          this.worker = this.view.state.facet(workerFactory)();
        } catch {
          this.worker = null;
        }
        if (this.worker) {
          this.worker.onmessage = (event) => {
            if (event.data.id === this.flight?.id) {
              this.deliver(event.data.notes);
            }
          };
          this.worker.onerror = () => {
            this.worker?.terminate();
            this.worker = null;
            this.broken = true;
            const lost = this.flight;
            this.flight = null;
            if (lost && !this.destroyed) {
              this.request();
            }
          };
        } else {
          this.broken = true;
        }
      }
      if (this.worker) {
        this.worker.postMessage({ id: flight.id, text: doc.toString() });
        return;
      }
      // No worker: the read happens a tick later, not inside this update.
      this.timer = setTimeout(() => {
        if (this.flight?.id !== flight.id) {
          return;
        }
        try {
          this.deliver(read(doc));
        } catch {
          // A read that fails is tried again on the next edit that needs it.
          this.flight = null;
        }
      }, 0);
    }

    deliver(notes: Notes) {
      const flight = this.flight;
      this.flight = null;
      if (!flight || this.destroyed) {
        return;
      }
      const current = this.view.state.field(model);
      if (flight.revision !== current.revision) {
        // An edit that can change footnotes came in; read again after a pause.
        this.later();
        return;
      }
      this.view.dispatch({
        effects: settled.of({
          revision: flight.revision,
          notes: flight.since.empty ? notes : carry(notes, flight.since),
        }),
      });
    }

    destroy() {
      this.destroyed = true;
      clearTimeout(this.timer);
      this.worker?.terminate();
      this.worker = null;
    }
  },
);

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
export function footnotes(options: FootnoteOptions = {}): Extension {
  return [
    options.worker ? workerFactory.of(options.worker) : [],
    model,
    settling,
    drawing,
    theme,
  ];
}
