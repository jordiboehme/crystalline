/**
 * Who is in the room, as a row of overlapping avatars in the editor's header.
 *
 * Each avatar wears the color the participant's caret, selection and name
 * flag are painted in inside the text, so a cursor on the page and a circle in
 * the header are recognisably the same person. The list itself carries the
 * names ("In this session: ..."), so a screen reader hears who is here without
 * walking the row; each avatar also holds its name as text for the same
 * reason, with the initial drawn for sighted readers only.
 *
 * An agent working in the document is one of the avatars rather than a notice
 * of its own: it is in here, it is changing the text under your cursor, and it
 * has a name and a color like anybody else. Its avatar shows a robot glyph
 * instead of an initial, so somebody watching a line appear knows whether a
 * colleague or an agent wrote it.
 */

import { Bot } from "lucide-react";
import type { ReactElement } from "react";

import { inkOn } from "./colors";
import type { CollabParticipant } from "./useCollabSession";

/**
 * What a chip's tooltip says: the name shown, with the login beside it when
 * the participant published one that differs, and "(you)" on your own.
 */
function chipTitle(one: CollabParticipant): string {
  const named =
    one.login !== undefined && one.login !== one.name
      ? `${one.name} (${one.login})`
      : one.name;
  return one.self ? `${named} (you)` : named;
}

/** The letter an avatar shows: the name's first letter, or a dot. */
function initialOf(name: string): string {
  const letter = /\p{L}|\p{N}/u.exec(name)?.[0];
  return letter === undefined ? "?" : letter.toUpperCase();
}

export function PresenceChips({
  participants,
  offline = false,
}: {
  participants: CollabParticipant[];
  /** True while the socket is down: who is where stopped being current the
   *  moment the awareness channel went quiet, so the row turns grey rather
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
      // Offline: grey rather than faded, so the initial keeps its
      // contrast on the avatar (fading both lowers it below 3:1).
      className={`flex items-center ${offline ? "grayscale" : ""}`}
    >
      {participants.map((one, index) => (
        <li
          // Two people may share a display name; the position in the room's
          // own ordering is what tells those avatars apart.
          key={`${one.name}:${String(index)}`}
          title={chipTitle(one)}
          data-self={one.self ? "true" : undefined}
          className="relative -ml-1.5 flex size-7 items-center justify-center rounded-full text-xs font-semibold ring-2 ring-canvas first:ml-0"
          style={{ backgroundColor: one.color, color: inkOn(one.color) }}
        >
          <span aria-hidden="true">
            {one.agent ? (
              <Bot size={14} strokeWidth={2} className="shrink-0" />
            ) : (
              initialOf(one.name)
            )}
          </span>
          <span className="sr-only">{one.name}</span>
        </li>
      ))}
    </ul>
  );
}
