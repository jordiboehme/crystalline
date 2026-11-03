/**
 * What the deletion check costs in a hard-wrapped paragraph (D24). The check
 * reads the whole paragraph (a pair can wrap onto the next line), so its
 * work may grow with the paragraph, but no faster than the paragraph: a
 * paragraph five times longer may cost about five times as much, never
 * twenty-five. The test counts work (the syntax nodes the tree walks stand
 * on, the characters the sweeps read, the results parsed and read) instead of
 * timing it, so it gives the same answer on a quiet and on a busy machine.
 */

import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { documentLanguage } from "../setup";
import { checkWork, checkedDeletion, deletionOutcome } from "./deleteCheck";
import type { DeleteOutcome } from "./deleteCheck";
import { treeWalkWork } from "./hiddenMarks";
import { hiddenMarkLayerOn } from "./mode";

/** About 52 characters with one pair of each kind. */
const LINE = "word **bold** and *em* with `code` and [link](u) end";
/** A line with a closer after punctuation: its space cannot go (c). */
const STUCK = "word **bold.** and *em* with `code` and [link](u) end";
/** A bold that wraps onto the next line, and the line it closes on. */
const WRAP_OPEN = "word and *em* with `code` and **a bold that";
const WRAP_CLOSE = "wraps** with [link](u) and more words end";

/** 2.1 kB, 10.6 kB and 42 kB in one paragraph. */
const SIZES = [40, 200, 800];

/** How much faster than the paragraph the work may grow: a little slack. */
const SLACK = 1.25;

/** A parsed state on `lines` lines in one paragraph, `middle` in the middle. */
function paragraph(lines: number, middle: string, fill: readonly string[]) {
  const half = Math.floor(lines / 2);
  const all = Array.from(
    { length: lines },
    (_, i) => fill[i % fill.length] ?? "",
  );
  all[half] = middle;
  const doc = all.join("\n");
  const raw = EditorState.create({
    doc,
    extensions: [documentLanguage, hiddenMarkLayerOn.of(true), checkedDeletion],
  });
  ensureSyntaxTree(raw, doc.length, 30_000);
  const state = raw.update({}).state;
  const lineStart = all
    .slice(0, half)
    .reduce((sum, line) => sum + line.length + 1, 0);
  return { state, lineStart };
}

interface Work {
  /** Nodes stood on and characters read, in all. */
  total: number;
  reads: number;
  outcome: DeleteOutcome | null;
}

/** The work one Backspace on `pos` costs the check. */
function work(state: EditorState, pos: number): Work {
  treeWalkWork.steps = 0;
  checkWork.positions = 0;
  checkWork.reads = 0;
  const tr = state.update({
    changes: { from: pos - 1, to: pos },
    userEvent: "delete.backward",
  });
  const effect = tr.effects.find((e) => e.is(deletionOutcome));
  return {
    total: treeWalkWork.steps + checkWork.positions,
    reads: checkWork.reads,
    outcome: effect ? effect.value : null,
  };
}

/** The work at each size, and the check that it grows at most linearly. */
function grows(at: (lines: number) => Work, outcome: DeleteOutcome | null) {
  const works = SIZES.map((lines) => at(lines));
  for (const one of works) expect(one.outcome).toBe(outcome);
  // As many results read at every size: no parse per line.
  expect(new Set(works.map((one) => one.reads)).size).toBe(1);
  for (let i = 1; i < SIZES.length; i += 1) {
    const size = (SIZES[i] ?? 0) / (SIZES[i - 1] ?? 1);
    const cost = (works[i]?.total ?? 0) / (works[i - 1]?.total ?? 1);
    expect(
      cost,
      `${String(SIZES[i - 1])} to ${String(SIZES[i])} lines: work x${cost.toFixed(1)} for size x${size.toFixed(1)} (${works.map((one) => String(one.total)).join(", ")})`,
    ).toBeLessThanOrEqual(size * SLACK);
  }
}

describe("the deletion check's work in a hard-wrapped paragraph (D24)", () => {
  it("grows at most linearly for an exact deletion", () => {
    grows((lines) => {
      // A letter of "word": the exact deletion.
      const { state, lineStart } = paragraph(lines, LINE, [LINE]);
      return work(state, lineStart + 2);
    }, null);
  });

  it("grows at most linearly for a refused deletion", () => {
    grows((lines) => {
      // The space after "**bold.**": every repair is tried, then refused.
      const { state, lineStart } = paragraph(lines, STUCK, [LINE]);
      return work(state, lineStart + STUCK.indexOf(" and") + 1);
    }, "refused");
  });

  it("grows at most linearly where every other line ends in a wrapped bold", () => {
    grows((lines) => {
      // A letter of "word" on a line between two wrapped pairs.
      const { state, lineStart } = paragraph(lines, LINE, [
        WRAP_OPEN,
        WRAP_CLOSE,
      ]);
      return work(state, lineStart + 2);
    }, null);
  });
});
