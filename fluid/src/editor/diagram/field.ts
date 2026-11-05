/**
 * The fences the layer draws, and the verdicts that decide them.
 *
 * A mermaid fence is drawn when it is top-level, closed, of an editable kind
 * and `verdictFor(body)` is `ok`. A verdict is asked for asynchronously
 * (`requestVerdict` loads mermaid), so there is a time when a fence has none:
 *
 * - a fence that was drawn before the transaction (found again by mapping its
 *   place through the changes) stays drawn while the verdict of its new body
 *   is pending, so typing in a drawn diagram never flashes the source;
 * - a fence that was never drawn stays text until its verdict arrives;
 * - a check that could not run (mermaid failed to load) is no verdict: it is
 *   not cached, the fence stays as it was, and the next edit asks again.
 *
 * Past where the parse reaches (a paste or an undo above a fence can leave the
 * new tree short) a drawn fence stays drawn at its mapped place until the tree
 * reaches it, as the grid keeps its tables (D58).
 *
 * A `StateField`, not a plugin: a block replacement may not come from a
 * plugin. The plugin beside it only asks for verdicts and reports the
 * settled ones with `verdictArrived`.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Range, Transaction } from "@codemirror/state";
import { MapMode, StateEffect, StateField } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";

import { parseAdvanced } from "../parseProgress";
import { diagramKindOf, EDITABLE_KINDS } from "./draws";
import { mermaidFences } from "./fence";
import { closeDiagramIntent } from "./intent";
import { requestVerdict, verdictFor } from "./verdict";

/** A fence the layer draws, at document positions. */
export interface DrawnFence {
  /** The fence's first mark and its last character, as `MermaidFence`. */
  from: number;
  to: number;
  body: string;
  kind: string;
}

/** A body whose verdict settled: the field draws again. */
export const verdictArrived = StateEffect.define<string>();

/**
 * The hook for a fence that may be drawn although the main selection is in
 * it (R52): Task 11's starter insert puts the caret inside the fence it just
 * wrote and still wants it drawn. The value is the fence's `from` in the
 * document the transaction produces. The exemption follows the fence through
 * changes and ends once the fence is drawn or is no fence any more.
 */
export const drawUnderCaret = StateEffect.define<number>();

interface DiagramFieldValue {
  fences: DrawnFence[];
  decorations: DecorationSet;
  /** Whether a fence with an ok verdict is withheld because the caret is in it. */
  held: boolean;
  /** `from` of the fences exempt from that hold (`drawUnderCaret`). */
  exempt: number[];
}

/** The placeholder until Task 9's widget: the skeleton box the fence became. */
class DiagramPlaceholder extends WidgetType {
  readonly body: string;

  constructor(body: string) {
    super();
    this.body = body;
  }

  override eq(other: DiagramPlaceholder): boolean {
    return other.body === this.body;
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("div");
    box.className = "cm-diagram";
    box.setAttribute("role", "group");
    box.setAttribute("aria-label", "Flowchart");
    return box;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

/** The fences of `state` the layer could draw, whatever their verdict. */
export function candidateFences(state: EditorState): DrawnFence[] {
  const found: DrawnFence[] = [];
  for (const fence of mermaidFences(state)) {
    if (!fence.topLevel) {
      continue;
    }
    const kind = diagramKindOf(fence.body);
    if (kind !== null && EDITABLE_KINDS.includes(kind)) {
      found.push({ from: fence.from, to: fence.to, body: fence.body, kind });
    }
  }
  return found;
}

/**
 * The fences drawn before `tr` that the tree does not reach yet, at their
 * mapped places: the ones to keep drawn (D58). Not those the tree reaches
 * (it decides), not those a change touched (their text is not what was
 * checked) and not those a candidate already covers.
 */
export function carryFences(
  previous: readonly DrawnFence[],
  tr: Transaction,
  treeLength: number,
  candidates: readonly DrawnFence[],
): DrawnFence[] {
  const carried: DrawnFence[] = [];
  for (const fence of previous) {
    const from = tr.changes.mapPos(fence.from, 1);
    const to = tr.changes.mapPos(fence.to, -1);
    if (
      to <= treeLength ||
      tr.changes.touchesRange(fence.from, fence.to) !== false ||
      candidates.some((c) => c.from <= to && from <= c.to)
    ) {
      continue;
    }
    carried.push({ ...fence, from, to });
  }
  return carried;
}

function decorate(state: EditorState, fences: readonly DrawnFence[]) {
  const doc = state.doc;
  const ranges: Range<Decoration>[] = fences.map((fence) =>
    Decoration.replace({
      block: true,
      widget: new DiagramPlaceholder(fence.body),
    }).range(doc.lineAt(fence.from).from, doc.lineAt(fence.to).to),
  );
  return Decoration.set(ranges, true);
}

function build(
  state: EditorState,
  wasDrawn: (from: number) => boolean,
  carried: (candidates: DrawnFence[]) => DrawnFence[],
  exempt: readonly number[],
): DiagramFieldValue {
  const candidates = candidateFences(state);
  const { from: head, to: tail } = state.selection.main;
  let held = false;
  const fences = candidates.filter((fence) => {
    const verdict = verdictFor(fence.body);
    if (wasDrawn(fence.from)) {
      // An edit keeps a drawn fence drawn, whatever its new verdict is until
      // it arrives; a refusal takes it back.
      return verdict === undefined || verdict.ok;
    }
    if (verdict === undefined || !verdict.ok) {
      return false;
    }
    // R52: typing into a fence must not take the source away under the caret.
    // It is drawn when the caret leaves (the field rebuilds on a selection
    // move while one is held), unless `drawUnderCaret` exempted it.
    if (
      head <= fence.to &&
      tail >= fence.from &&
      !exempt.includes(fence.from)
    ) {
      held = true;
      return false;
    }
    return true;
  });
  fences.push(...carried(candidates));
  fences.sort((a, b) => a.from - b.from);
  const drawn = new Set(fences.map((fence) => fence.from));
  return {
    fences,
    decorations: decorate(state, fences),
    held,
    exempt: exempt.filter(
      (from) =>
        !drawn.has(from) && candidates.some((fence) => fence.from === from),
    ),
  };
}

function arrived(tr: Transaction): boolean {
  return tr.effects.some((effect) => effect.is(verdictArrived));
}

/**
 * Where the fence at `from` is after `changes`, or null when a change removed
 * or replaced it: an insert at `from` and an edit inside the body keep it
 * (`TrackAfter` asks whether the character after `from` survives).
 */
function placeAfter(tr: Transaction, from: number): number | null {
  return tr.changes.mapPos(from, 1, MapMode.TrackAfter);
}

export const diagramField = StateField.define<DiagramFieldValue>({
  create: (state) =>
    build(
      state,
      () => false,
      () => [],
      [],
    ),
  update: (value, tr) => {
    const exemptNow = [
      ...value.exempt.flatMap((from) => placeAfter(tr, from) ?? []),
      ...tr.effects.flatMap((effect) =>
        effect.is(drawUnderCaret) ? [effect.value] : [],
      ),
    ];
    if (
      !tr.docChanged &&
      !parseAdvanced(tr) &&
      !arrived(tr) &&
      !(value.held && tr.selection !== undefined) &&
      exemptNow.length === value.exempt.length &&
      !tr.effects.some((effect) => effect.is(drawUnderCaret))
    ) {
      return value;
    }
    const before = new Set(
      value.fences.flatMap((f) => placeAfter(tr, f.from) ?? []),
    );
    return build(
      tr.state,
      (from) => before.has(from),
      (candidates) =>
        carryFences(value.fences, tr, syntaxTree(tr.state).length, candidates),
      exemptNow,
    );
  },
  provide: (field) => [
    EditorView.decorations.from(field, (value) => value.decorations),
    // The drawn block is one unit to the caret and to deletion.
    EditorView.atomicRanges.of(
      (view) => view.state.field(field, false)?.decorations ?? Decoration.none,
    ),
  ],
});

/**
 * Asks for every missing verdict, reports the settled ones, and tells the
 * intent when a fence it had drawn is no longer drawn.
 */
export const verdictWatcher = ViewPlugin.fromClass(
  class {
    /**
     * Bodies this view asked about. A body leaves it only when its check
     * could not run; otherwise it is never asked again, so a cache that
     * evicts verdicts (256 entries, shared by every view) cannot start the
     * asking over.
     */
    readonly asked = new Set<string>();
    gone = false;
    readonly view: EditorView;

    constructor(view: EditorView) {
      this.view = view;
      this.ask();
    }

    update(update: ViewUpdate): void {
      const edited = update.docChanged || parseAdvanced(update);
      if (!edited && !update.transactions.some(arrived)) {
        return;
      }
      // A settled verdict (`verdictArrived`) is no reason to ask again.
      if (edited) {
        this.ask();
      }
      const now = new Set(
        update.state.field(diagramField).fences.map((fence) => fence.from),
      );
      const lost = update.startState
        .field(diagramField)
        .fences.some((fence) => {
          const at = update.changes.mapPos(fence.from, 1, MapMode.TrackAfter);
          return at === null || !now.has(at);
        });
      if (lost) {
        // Not inside the update: the real teardown dispatches.
        queueMicrotask(() => {
          if (!this.gone) {
            closeDiagramIntent(this.view, "gone");
          }
        });
      }
    }

    ask(): void {
      const { state } = this.view;
      const dark = state.facet(EditorView.darkTheme);
      for (const { body } of candidateFences(state)) {
        if (verdictFor(body) !== undefined || this.asked.has(body)) {
          continue;
        }
        this.asked.add(body);
        requestVerdict(body, dark).then(
          () => {
            // A check that could not run left no verdict: nothing to report,
            // and the next edit asks again.
            if (verdictFor(body) === undefined) {
              this.asked.delete(body);
            } else if (!this.gone) {
              this.view.dispatch({ effects: verdictArrived.of(body) });
            }
          },
          () => {
            this.asked.delete(body);
          },
        );
      }
    }

    destroy(): void {
      this.gone = true;
    }
  },
);

export const diagramTheme = EditorView.baseTheme({
  ".cm-diagram": {
    minHeight: "3rem",
    padding: "0.5rem 0.75rem",
  },
});
