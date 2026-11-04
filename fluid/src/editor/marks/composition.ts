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
  EditorState as State,
  Prec,
  StateEffect,
  StateField,
  Transaction,
} from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin, keymap } from "@codemirror/view";

import { mainSelectionParked } from "../parkedSelection";
import { composing } from "./caret";
import { commandJudge } from "./commandJudge";
import { carried, deletionOutcome, judgeDeletion, placed } from "./deleteCheck";
import { judgedElsewhere } from "./deleting";
import { pendingFormat, setPending, typedPlan } from "./formatting";
import { judgeInsert } from "./insertCheck";
import { PARSE_BUDGET, deadlineIn } from "./parse";

/** What one composition changed, from the state before it began. */
interface CompositionLog {
  start: EditorState;
  changes: ChangeSet;
  /** The selection the composition's last change left. */
  selection: EditorSelection;
}

/** Ends the log: the composition was judged, or is not to be. */
const endLog = StateEffect.define<null>();

/** The events a waiting composition is judged before (D56, D57). */
const FIRST_EVENTS = [
  "compositionstart",
  "mousedown",
  "paste",
  "drop",
  "cut",
] as const;

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
        ? {
            start: log.start,
            changes: log.changes.compose(tr.changes),
            selection: tr.newSelection,
          }
        : {
            start: tr.startState,
            changes: tr.changes,
            selection: tr.newSelection,
          };
    }
    if (log === null || !tr.docChanged) {
      return log;
    }
    // The last composed change can come as plain typing just after the
    // composition ended: it is part of what is judged.
    if (local && tr.isUserEvent("input.type")) {
      return {
        start: log.start,
        changes: log.changes.compose(tr.changes),
        selection: tr.newSelection,
      };
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
    return typedWanted(start, net.from, text);
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
 * Text composed at a caret, by the typing rules on the state from before
 * the composition. Whitespace at its end (the space a Korean keyboard types
 * with the key that commits a syllable) is typed after the rest, so it goes
 * outside the marks of a format that waits, as it does when typed after a
 * pause (D48).
 */
function typedWanted(
  start: EditorState,
  from: number,
  text: string,
): Wanted | null {
  const deadline = deadlineIn(PARSE_BUDGET.typing);
  const at =
    start.selection.main.empty && start.selection.main.head === from
      ? start
      : start.update({ selection: EditorSelection.cursor(from) }).state;
  const core = text.replace(/\s+$/, "");
  const tail = text.slice(core.length);
  if (core === "" || tail === "") {
    const plan = typedPlan(at, from, text, deadline);
    return plan
      ? {
          changes: start.changes({ from: plan.at, insert: plan.insert }),
          caret: plan.caret,
          effects: [setPending.of(plan.pending)],
        }
      : null;
  }
  const first = typedPlan(at, from, core, deadline);
  const firstChanges = start.changes(
    first ? { from: first.at, insert: first.insert } : { from, insert: core },
  );
  const caret = first ? first.caret : from + core.length;
  const mid = start.update({
    changes: firstChanges,
    selection: EditorSelection.cursor(caret),
    effects: setPending.of(first ? first.pending : null),
    filter: false,
  }).state;
  const second = typedPlan(mid, caret, tail, deadline);
  const secondChanges = mid.changes(
    second
      ? { from: second.at, insert: second.insert }
      : { from: caret, insert: tail },
  );
  if (!first && !second) {
    return null;
  }
  return {
    changes: firstChanges.compose(secondChanges),
    caret: second ? second.caret : caret + tail.length,
    effects: [setPending.of(second ? second.pending : null)],
  };
}

/** The correction a log asks for, as a change of the state now. */
interface Correction {
  changes: ChangeSet;
  /** The caret the judges plan, or null to keep the caret as it maps. */
  caret: number | null;
  effects: StateEffect<unknown>[];
}

/**
 * What the composition `now`'s log holds asks for, on `now`: the change from
 * the text now to the text the judges want, the caret they plan and what is
 * said, with the log ended. Null when no log is open.
 */
function correctionOf(now: EditorState): Correction | null {
  const log = now.field(compositionLog, false) ?? null;
  if (log === null) {
    return null;
  }
  const want: Wanted | null = wanted(log.start, now, log);
  const effects: StateEffect<unknown>[] = [endLog.of(null)];
  if (want === null || want.changes === null) {
    if ((now.field(pendingFormat, false) ?? null) !== null) {
      // A format that waited through the composition and was not written.
      effects.push(setPending.of(null));
    }
    return { changes: now.changes([]), caret: null, effects };
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
  const changes = now.changes(
    head === current.length && head === goal.length
      ? []
      : [
          {
            from: head,
            to: current.length - tail,
            insert: goal.slice(head, goal.length - tail),
          },
        ],
  );
  return {
    changes,
    caret: Math.min(want.caret ?? head, goal.length),
    effects: [...effects, ...want.effects],
  };
}

/** The part of CodeMirror's DOM observer the judge reads. Internal. */
interface DomObserver {
  delayedFlush: unknown;
  delayedAndroidKey: unknown;
}

/**
 * Whether CodeMirror holds DOM changes it has not read yet. Chrome for
 * Android below 126 (no EditContext) delays the flush after
 * `compositionend` to the next animation frame, and the last composed text
 * waits there. A dispatch before that flush makes CodeMirror throw the
 * waiting changes away and draw over them, so the text is lost (D59).
 */
export function domPending(view: EditorView): boolean {
  const observer = (view as unknown as { observer?: DomObserver }).observer;
  if (!observer) return false;
  // Only what CodeMirror itself holds back. Records its observer has not
  // delivered yet are not asked for: taking them would keep them from the
  // flush that reads them.
  // A number only: `null >= 0` is true, and a CodeMirror that kept null for
  // "no delayed flush" would hold every judge back for good (R4-3).
  const { delayedFlush } = observer;
  return (
    (typeof delayedFlush === "number" && delayedFlush >= 0) ||
    Boolean(observer.delayedAndroidKey)
  );
}

/**
 * Judges the composition the view's log holds, once it is over: writes the
 * correction, or just ends the log. Never from inside an update, and never
 * while CodeMirror holds DOM changes it has not read (D59): the log then
 * waits, and the settle timer or the next key judges it.
 */
export function judgeComposition(view: EditorView): void {
  if (view.composing || view.compositionStarted || domPending(view)) {
    return;
  }
  const log = view.state.field(compositionLog, false) ?? null;
  const fix = correctionOf(view.state);
  if (fix === null) {
    return;
  }
  // A selection the person made after the composition stays theirs,
  // mapped through the correction (D51): a toolbar command right after
  // acts on it (D60).
  const moved =
    log !== null && !view.state.selection.eq(log.selection)
      ? view.state.selection.map(fix.changes)
      : null;
  if (fix.changes.empty && (fix.caret === null || moved !== null)) {
    view.dispatch({ effects: fix.effects });
    return;
  }
  const selection =
    moved ?? (fix.caret === null ? null : EditorSelection.cursor(fix.caret));
  view.dispatch({
    changes: fix.changes,
    ...(selection === null ? {} : { selection }),
    effects: fix.effects,
    // Joins the composition's undo step, solo and in a room.
    userEvent: "input.type",
    annotations: judgedElsewhere.of(true),
    scrollIntoView: true,
  });
}

/** A waiting composition, judged before a command from outside (D60). */
function judgeWaiting(view: EditorView): void {
  if ((view.state.field(compositionLog, false) ?? null) !== null) {
    judgeComposition(view);
  }
}

/**
 * An action no DOM event announced (a toolbar command, a popover commit, a
 * test's dispatch) while a composition waits to be judged: the composition
 * is judged first, on the state before the action, in the same transaction
 * (D47). Keys, presses, pastes, drops, cuts and a new composition are judged
 * before they reach CodeMirror (`judgeBeforeKey`, the capture listeners in
 * `settle`), in a transaction of their own that joins the composition's undo
 * step (D57). The action follows, mapped through the correction, with the
 * selection it set (a caret the person moved stays where they put it, D51).
 * Typing, composed or plain, is part of the composition (CodeMirror can
 * deliver the last composed change that way), so nothing is written while
 * the IME composes; a peer's change ends the log unjudged (the D33 limit).
 *
 * Listed after the deletion and insert checks, so it runs after them: they
 * judge the action itself, and this one only puts the composition's
 * correction in front of it.
 */
export const judgeFirst: Extension = State.transactionFilter.of((tr) => {
  const start = tr.startState;
  if ((start.field(compositionLog, false) ?? null) === null) {
    return tr;
  }
  const event = tr.annotation(Transaction.userEvent);
  if (
    event === undefined ||
    tr.annotation(judgedElsewhere) === true ||
    tr.annotation(Transaction.remote) !== undefined ||
    tr.isUserEvent("input.type") ||
    composing(start) ||
    (!tr.docChanged && tr.selection === undefined)
  ) {
    return tr;
  }
  const fix = correctionOf(start);
  if (fix === null) {
    return tr;
  }
  const rest = tr.changes.map(fix.changes);
  const effects = StateEffect.mapEffects(
    StateEffect.mapEffects(tr.effects, tr.changes.invertedDesc),
    fix.changes,
  );
  const selection = tr.selection
    ? tr.newSelection.map(tr.changes.invertedDesc).map(fix.changes).map(rest)
    : fix.caret === null
      ? undefined
      : EditorSelection.cursor(fix.caret).map(rest);
  return [
    { changes: fix.changes, effects: fix.effects },
    {
      changes: rest,
      ...(selection ? { selection } : {}),
      effects: StateEffect.mapEffects(effects, rest),
      annotations: carried(tr),
      scrollIntoView: tr.scrollIntoView,
      sequential: true,
    },
  ];
});

const settle = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | null = null;

    readonly view: EditorView;

    /** Judges a waiting composition before CodeMirror hears the event. */
    readonly first = (): void => {
      if (
        (this.view.state.field(compositionLog, false) ?? null) !== null &&
        // Changes CodeMirror has not read yet: the settle timer judges once
        // they are in (D59).
        !domPending(this.view)
      ) {
        this.cancel();
        judgeComposition(this.view);
      }
    };

    constructor(view: EditorView) {
      this.view = view;
      // Capture on the editor's outer element: before CodeMirror's own
      // handlers on the content, and before the browser writes the next
      // composition's first text (D56). A press, a paste, a drop and a cut
      // are judged there too, so the correction is a step of the
      // composition's own, apart from the action (D57).
      for (const type of FIRST_EVENTS) {
        view.dom.addEventListener(type, this.first, true);
      }
    }

    schedule(ms: number): void {
      this.cancel();
      this.timer = setTimeout(() => {
        this.timer = null;
        if (domPending(this.view)) {
          // The last composed change is still on its way in (D59).
          this.schedule(SETTLE_MS);
          return;
        }
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
      }
    }

    destroy(): void {
      this.cancel();
      for (const type of FIRST_EVENTS) {
        this.view.dom.removeEventListener(type, this.first, true);
      }
    }
  },
  {
    eventHandlers: {
      compositionend() {
        // The last composed change may come up to 50 ms after the event.
        this.schedule(SETTLE_MS);
      },
    },
  },
);

/**
 * A key pressed while a composition waits to be judged: the composition is
 * judged first, so the key's own command reads the text as the judges want
 * it (a Backspace takes the letter the person sees, D47). Keys an IME still
 * handles (a composition under way) are left alone. First in the layer:
 * CodeMirror runs a key's `any` bindings in the order the keymaps came.
 */
export const judgeBeforeKey = Prec.highest(
  keymap.of([
    {
      any: (view, event) => {
        if (
          !event.isComposing &&
          event.keyCode !== 229 &&
          (view.state.field(compositionLog, false) ?? null) !== null
        ) {
          judgeComposition(view);
        }
        return false;
      },
    },
  ]),
);

/** What the hidden-mark layer installs for compositions. */
export const compositionCheck: Extension = [
  compositionLog,
  settle,
  commandJudge.of(judgeWaiting),
];
