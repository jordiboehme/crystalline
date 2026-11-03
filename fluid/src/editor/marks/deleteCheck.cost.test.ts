/**
 * What the deletion check costs in a hard-wrapped paragraph. The check reads
 * the whole paragraph (a pair can wrap onto the next line), so its cost must
 * stay a small multiple of what a plain delete already pays there: the
 * parse of that paragraph again. Measured as a ratio of medians over
 * interleaved presses, so a loaded machine slows both sides alike.
 */

import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { documentLanguage } from "../setup";
import { checkedDeletion, deletionOutcome } from "./deleteCheck";
import type { DeleteOutcome } from "./deleteCheck";
import { hiddenMarkLayerOn } from "./mode";

/** About 52 characters with one pair of each kind. */
const LINE = "word **bold** and *em* with `code` and [link](u) end";
/** A line with a closer after punctuation: its space cannot go (c). */
const STUCK = "word **bold.** and *em* with `code` and [link](u) end";

/** The ratio a checked press may cost over a plain one. */
const MAX_RATIO = 10;
const ROUNDS = 15;

function median(values: readonly number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)] ?? 0;
}

/** A parsed state on `lines` lines in one paragraph, middle line `middle`. */
function paragraph(lines: number, middle: string) {
  const all = Array.from({ length: lines }, (_, i) =>
    i === Math.floor(lines / 2) ? middle : LINE,
  );
  const doc = all.join("\n");
  const raw = EditorState.create({
    doc,
    extensions: [documentLanguage, hiddenMarkLayerOn.of(true), checkedDeletion],
  });
  ensureSyntaxTree(raw, doc.length, 10_000);
  const state = raw.update({}).state;
  const lineStart = all
    .slice(0, Math.floor(lines / 2))
    .reduce((sum, line) => sum + line.length + 1, 0);
  return { state, lineStart };
}

/** The time of one delete of `from..to` as `userEvent`, parse included. */
function press(
  state: EditorState,
  from: number,
  userEvent: string,
): { ms: number; outcome: DeleteOutcome | null } {
  const begin = performance.now();
  const tr = state.update({ changes: { from, to: from + 1 }, userEvent });
  ensureSyntaxTree(tr.state, tr.state.doc.length, 10_000);
  const ms = performance.now() - begin;
  const effect = tr.effects.find((e) => e.is(deletionOutcome));
  return { ms, outcome: effect ? effect.value : null };
}

/** Checked over plain, as a ratio of medians of interleaved presses. */
function ratio(state: EditorState, from: number) {
  const checked: number[] = [];
  const plain: number[] = [];
  let outcome: DeleteOutcome | null = null;
  for (let i = 0; i < ROUNDS; i += 1) {
    const one = press(state, from, "delete.backward");
    outcome = one.outcome;
    checked.push(one.ms);
    // Not a key the check judges: the same change, parsed the same way.
    plain.push(press(state, from, "input.plain").ms);
  }
  const c = median(checked);
  const p = median(plain);
  return { ratio: c / Math.max(p, 0.05), checked: c, plain: p, outcome };
}

describe("the deletion check's cost in a hard-wrapped paragraph", () => {
  it.each([40, 200])(
    "stays a small multiple of a plain delete, %i lines",
    (lines) => {
      // A letter of "word": the exact deletion.
      const exact = paragraph(lines, LINE);
      const letter = ratio(exact.state, exact.lineStart + 1);
      expect(letter.outcome).toBeNull();
      // The space after "**bold.**": every repair is tried, then refused.
      const stuck = paragraph(lines, STUCK);
      const space = ratio(stuck.state, stuck.lineStart + STUCK.indexOf(" and"));
      expect(space.outcome).toBe("refused");
      console.info(
        `${String(lines)} lines (${String(exact.state.doc.length)} B): exact ${letter.checked.toFixed(2)} ms vs ${letter.plain.toFixed(2)} ms (x${letter.ratio.toFixed(1)}), refused ${space.checked.toFixed(2)} ms vs ${space.plain.toFixed(2)} ms (x${space.ratio.toFixed(1)})`,
      );
      expect(letter.ratio).toBeLessThan(MAX_RATIO);
      expect(space.ratio).toBeLessThan(MAX_RATIO);
    },
    60_000,
  );
});
