/**
 * An IME composition, judged when it ends (I3, D33, D34). While the browser
 * composes, nothing is written beside the composed text: a write there could
 * end the composition. So the keys' checks stand back (`checkedDeletion`,
 * `checkedInsert`, the typing rules, the caret normalizer), and this module
 * keeps a log of what the composition changed and judges the whole of it at
 * its end, with the same rules a key gets:
 *
 * - text composed where a format waits (Bold, then a word on a phone) gets
 *   that format, and text composed at a format's edge follows the typing
 *   rules (`typedPlan`), so `**red** ` and a composed `sea` give
 *   `**red** **sea**` as on a desktop;
 * - a composed deletion (Gboard writes `red` over `red` as `re`) goes to the
 *   deletion judge (`judgeDeletion`): exact, repaired, or refused, in which
 *   case the deleted text comes back and the page says so;
 * - a composed replacement goes to the insert judge (`judgeInsert`).
 *
 * The correction is one `input.type` transaction right after the
 * composition, so it joins the composition's undo step, solo and in a room:
 * one undo gives back the text from before the composition.
 *
 * Limits: a peer's change, a reconfigure or a parked selection during the
 * composition drops the log, and that composition goes unjudged. It is rare,
 * and the peer sees the result.
 */

import type { ChangeSet, EditorState, Extension } from "@codemirror/state";
import {
  EditorSelection,
  StateEffect,
  StateField,
  Transaction,
} from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin } from "@codemirror/view";

import { mainSelectionParked } from "../parkedSelection";
import { deletionOutcome, judgeDeletion, placed } from "./deleteCheck";
import { judgedElsewhere } from "./deleting";
import { pendingFormat, setPending, typedPlan } from "./formatting";
import { judgeInsert } from "./insertCheck";
import { PARSE_BUDGET, deadlineIn } from "./parse";

/** What one composition changed, from the state before it began. */
interface CompositionLog {
  start: EditorState;
  changes: ChangeSet;
}

/** Ends the log: the composition was judged, or is not to be. */
const endLog = StateEffect.define<null>();

/** How long after `compositionend` the last composed change may arrive. */
const SETTLE_MS = 60;

const compositionLog = StateField.define<CompositionLog | null>({
  create: () => null,
  update(log, tr) {
    if (tr.effects.some((effect) => effect.is(endLog))) {
      return null;
    }
    if (tr.reconfigured || tr.state.facet(mainSelectionParked)) {
      return null;
    }
    const local =
      tr.annotation(Transaction.remote) === undefined &&
      tr.annotation(judgedElsewhere) !== true;
    if (tr.isUserEvent("input.type.compose") && local) {
      if (!tr.docChanged) {
        return log;
      }
      return log
        ? { start: log.start, changes: log.changes.compose(tr.changes) }
        : { start: tr.startState, changes: tr.changes };
    }
    if (log === null || !tr.docChanged) {
      return log;
    }
    // The last composed change can come as plain typing just after the
    // composition ended: it is part of what is judged.
    if (local && tr.isUserEvent("input.type")) {
      return { start: log.start, changes: log.changes.compose(tr.changes) };
    }
    // A peer's edit, or any other change: this composition goes unjudged.
    return null;
  },
});

/**
 * The composition's net change, reduced to one replacement: what it took
 * out of `start` and put in, at `from`. Null when it changed nothing.
 */
function netChange(
  start: EditorState,
  now: EditorState,
  changes: ChangeSet,
): { from: number; toA: number; toB: number } | null {
  let fromA = Infinity;
  let toA = -Infinity;
  let fromB = Infinity;
  let toB = -Infinity;
  changes.iterChangedRanges((a0, a1, b0, b1) => {
    fromA = Math.min(fromA, a0);
    toA = Math.max(toA, a1);
    fromB = Math.min(fromB, b0);
    toB = Math.max(toB, b1);
  });
  if (fromA === Infinity) {
    return null;
  }
  // Outside the changed ranges the two texts agree: trim what agrees inside.
  const before = start.doc.sliceString(fromA, toA);
  const after = now.doc.sliceString(fromB, toB);
  let head = 0;
  while (
    head < before.length &&
    head < after.length &&
    before[head] === after[head]
  ) {
    head += 1;
  }
  let tail = 0;
  while (
    tail < before.length - head &&
    tail < after.length - head &&
    before[before.length - 1 - tail] === after[after.length - 1 - tail]
  ) {
    tail += 1;
  }
  if (head === before.length && head === after.length) {
    return null;
  }
  return {
    from: fromA + head,
    toA: toA - tail,
    toB: fromB + (after.length - tail),
  };
}

/** What the text should read after the composition, by the judges. */
interface Wanted {
  /** The text the judges want, as a change on the composition's start. */
  changes: ChangeSet | null;
  caret: number | null;
  effects: StateEffect<unknown>[];
}

function wanted(start: EditorState, now: EditorState, log: CompositionLog) {
  const net = netChange(start, now, log.changes);
  if (net === null) {
    return null;
  }
  const removed = { from: net.from, to: net.toA };
  const text = now.doc.sliceString(net.from, net.toB);
  if (text === "") {
    const deletion = start.changes(removed);
    const judged = judgeDeletion(start, deletion, deadlineIn(PARSE_BUDGET.key));
    const head = now.selection.main.head;
    const back = deletion.invertedDesc.mapPos(head, -1);
    switch (judged.outcome) {
      case "unchecked":
        return null;
      case "exact":
        return judged.changes
          ? {
              changes: judged.changes,
              caret: placed(back, judged.changes, [], undefined),
              effects: [],
            }
          : null;
      case "refused":
        // The deleted text comes back, and the page says why.
        return {
          changes: start.changes([]),
          caret: start.selection.main.head,
          effects: [
            EditorView.announce.of(judged.said),
            deletionOutcome.of("refused"),
          ],
        };
      default:
        return {
          changes: judged.changes,
          caret: placed(back, judged.changes, judged.ranges, judged.moved),
          effects: [deletionOutcome.of(judged.outcome)],
        };
    }
  }
  if (removed.from === removed.to) {
    const at =
      start.selection.main.empty && start.selection.main.head === net.from
        ? start
        : start.update({ selection: EditorSelection.cursor(net.from) }).state;
    const plan = typedPlan(at, net.from, text, deadlineIn(PARSE_BUDGET.typing));
    if (!plan) {
      return null;
    }
    return {
      changes: start.changes({ from: plan.at, insert: plan.insert }),
      caret: plan.caret,
      effects: [setPending.of(plan.pending)],
    };
  }
  const judged = judgeInsert(
    start,
    [],
    { from: removed.from, to: removed.to, text },
    deadlineIn(PARSE_BUDGET.key),
  );
  return judged
    ? { changes: judged.changes, caret: judged.end, effects: [] }
    : null;
}

/**
 * Judges the composition the view's log holds, once it is over: writes the
 * correction, or just ends the log. Never from inside an update.
 */
export function judgeComposition(view: EditorView): void {
  if (view.composing || view.compositionStarted) {
    return;
  }
  const log = view.state.field(compositionLog, false) ?? null;
  if (log === null) {
    return;
  }
  const now = view.state;
  const want: Wanted | null = wanted(log.start, now, log);
  const effects: StateEffect<unknown>[] = [endLog.of(null)];
  if (want === null || want.changes === null) {
    if ((now.field(pendingFormat, false) ?? null) !== null) {
      // A format that waited through the composition and was not written.
      effects.push(setPending.of(null));
    }
    view.dispatch({ effects });
    return;
  }
  const target = want.changes.apply(log.start.doc);
  // The correction: the one stretch where the text now and the text wanted
  // differ.
  const current = now.doc.toString();
  const goal = target.toString();
  let head = 0;
  while (
    head < current.length &&
    head < goal.length &&
    current[head] === goal[head]
  ) {
    head += 1;
  }
  let tail = 0;
  while (
    tail < current.length - head &&
    tail < goal.length - head &&
    current[current.length - 1 - tail] === goal[goal.length - 1 - tail]
  ) {
    tail += 1;
  }
  const changes =
    head === current.length && head === goal.length
      ? []
      : [
          {
            from: head,
            to: current.length - tail,
            insert: goal.slice(head, goal.length - tail),
          },
        ];
  const caret = Math.min(want.caret ?? head, goal.length);
  view.dispatch({
    changes,
    selection: EditorSelection.cursor(caret),
    effects: [...effects, ...want.effects],
    // Joins the composition's undo step, solo and in a room.
    userEvent: "input.type",
    annotations: judgedElsewhere.of(true),
    scrollIntoView: true,
  });
}

const settle = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | null = null;

    readonly view: EditorView;

    constructor(view: EditorView) {
      this.view = view;
    }

    schedule(ms: number): void {
      this.cancel();
      this.timer = setTimeout(() => {
        this.timer = null;
        judgeComposition(this.view);
      }, ms);
    }

    cancel(): void {
      if (this.timer !== null) {
        clearTimeout(this.timer);
        this.timer = null;
      }
    }

    update(update: ViewUpdate): void {
      // A composition that ended without a `compositionend` this plugin saw
      // (a phone keyboard's commit can end it from the input itself): judge
      // it once the view stops composing.
      if (
        this.timer === null &&
        !this.view.composing &&
        !this.view.compositionStarted &&
        (update.state.field(compositionLog, false) ?? null) !== null
      ) {
        this.schedule(SETTLE_MS);
        return;
      }
      // A change that is not the composition's, after it ended: judge the
      // composition first, as soon as this update is over.
      if (
        this.timer !== null &&
        update.transactions.some(
          (tr) =>
            (tr.docChanged || tr.selection !== undefined) &&
            !tr.isUserEvent("input.type") &&
            tr.annotation(judgedElsewhere) !== true,
        )
      ) {
        this.schedule(0);
      }
    }

    destroy(): void {
      this.cancel();
    }
  },
  {
    eventHandlers: {
      compositionend() {
        // The last composed change may come up to 50 ms after the event.
        this.schedule(SETTLE_MS);
      },
      compositionstart() {
        // A new composition: the last one is judged before it begins.
        if (this.timer !== null) {
          this.cancel();
          judgeComposition(this.view);
        }
      },
    },
  },
);

/** What the hidden-mark layer installs for compositions. */
export const compositionCheck: Extension = [compositionLog, settle];
