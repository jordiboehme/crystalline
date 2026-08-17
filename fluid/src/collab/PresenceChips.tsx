/**
 * Who else is in the room, as a row of name chips beside the buffer.
 *
 * The colored dot is the same color the participant's caret and selection are
 * painted in inside the text, so a cursor in the margin of somebody's screen
 * and the chip in the header are recognisably the same person. The chips are
 * a list rather than a paragraph, and the list itself carries the names, so a
 * screen reader hears who is here without walking the row item by item.
 */

import type { ReactElement } from "react";

import type { CollabParticipant } from "./useCollabSession";

export function PresenceChips({
  participants,
  offline = false,
}: {
  participants: CollabParticipant[];
  /** True while the socket is down: who is where stopped being current the
   *  moment the awareness channel went quiet, so the row is dimmed rather
   *  than emptied - those people have not left, this tab just cannot hear
   *  them. */
  offline?: boolean;
}): ReactElement | null {
  if (participants.length === 0) {
    return null;
  }
  const names = participants
    .map((one) => (one.self ? `${one.name} (you)` : one.name))
    .join(", ");
  return (
    <ul
      aria-label={`In this session: ${names}`}
      className={
        offline
          ? "flex flex-wrap items-center gap-2 opacity-50"
          : "flex flex-wrap items-center gap-2"
      }
    >
      {participants.map((one, index) => (
        <li
          // Two people may share a display name; the position in the room's
          // own ordering is what tells those chips apart.
          key={`${one.name}:${String(index)}`}
          className="flex items-center gap-1.5 rounded-full border border-slate-300 px-2 py-0.5 text-xs text-slate-700 dark:border-slate-700 dark:text-slate-200"
        >
          <span
            aria-hidden="true"
            className="size-2 rounded-full"
            style={{ backgroundColor: one.color }}
          />
          {one.name}
          {one.self && (
            <span className="text-slate-500 dark:text-slate-400">you</span>
          )}
        </li>
      ))}
    </ul>
  );
}
