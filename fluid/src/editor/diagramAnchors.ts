/**
 * Where a diagram op's fence is while the op waits: verifying a result is
 * asynchronous, and a remote edit or a keystroke can arrive in that gap. A
 * tracked position is mapped through every change, and one a deletion ran
 * across is dropped, so a fence that was deleted can never read as the fence
 * that moved up into its place. An insert exactly at the position goes before
 * it.
 *
 * Eager and outside the preview compartment: Raw turning on while an op waits
 * must still find the fence.
 */

import { MapMode, StateEffect, StateField } from "@codemirror/state";

/** Starts tracking `pos` (a position in the document after this transaction). */
export const trackAnchor = StateEffect.define<{
  ticket: number;
  pos: number;
}>();

/** Stops tracking a ticket. */
export const untrackAnchor = StateEffect.define<number>();

/** Each ticket's position now; a ticket that is not here has lost its place. */
export const diagramAnchors = StateField.define<ReadonlyMap<number, number>>({
  create: () => new Map(),
  update(value, tr) {
    let next = value;
    if (tr.docChanged && value.size > 0) {
      const mapped = new Map<number, number>();
      for (const [ticket, pos] of value) {
        const to = tr.changes.mapPos(pos, 1, MapMode.TrackDel);
        if (to !== null) {
          mapped.set(ticket, to);
        }
      }
      next = mapped;
    }
    for (const effect of tr.effects) {
      if (effect.is(trackAnchor)) {
        next = new Map(next).set(effect.value.ticket, effect.value.pos);
      } else if (effect.is(untrackAnchor)) {
        const without = new Map(next);
        without.delete(effect.value);
        next = without;
      }
    }
    return next;
  },
});
