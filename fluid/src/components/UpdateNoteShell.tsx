/**
 * The popover frame of the update note (spec S28-S31), and the short line it
 * holds when the note's own chunk cannot load.
 *
 * Anchored to the avatar rather than to the chip: the chip is inside the menu,
 * which closes when the chip is chosen. There is no popover trigger, so a
 * close by Escape hands the focus back to the avatar by hand. A close by a
 * click or Tab elsewhere leaves the focus where the reader put it.
 *
 * In the entry, not lazy: the fallback must draw when the lazy note cannot
 * (a tab left open across `crystalline update now` names chunks the new
 * server no longer has).
 */

import { Popover } from "radix-ui";
import type { ReactElement, ReactNode, RefObject } from "react";

import { FOCUS_RING } from "./primitives";

/** What the note's frame needs to open and to close. */
export interface UpdateNoteFrame {
  /** The avatar: where the popover opens and where the focus goes back to. */
  anchor: RefObject<HTMLButtonElement | null>;
  /** The newer version, already checked to be a plain `x.y.z`. */
  available: string;
  onClose: () => void;
}

export function UpdateNoteShell({
  anchor,
  available,
  onClose,
  children,
}: UpdateNoteFrame & { children: ReactNode }): ReactElement {
  return (
    <Popover.Root
      open
      onOpenChange={(open) => {
        if (!open) {
          onClose();
        }
      }}
    >
      <Popover.Anchor virtualRef={anchor} />
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          collisionPadding={8}
          aria-label={`Crystalline ${available} available`}
          className="menu-face w-72 space-y-2.5 p-3 leading-snug"
          onCloseAutoFocus={(event) => {
            // Radix would hand the focus to a trigger, and there is none.
            // The focus goes back to the avatar only when the close left it
            // nowhere (Escape inside the note); a click or Tab elsewhere has
            // already put it where the reader wants it.
            event.preventDefault();
            const active = document.activeElement;
            if (active === null || active === document.body) {
              anchor.current?.focus();
            }
          }}
        >
          {children}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** The note when its chunk did not load: one line and a reload. */
export function UpdateNoteFailed(frame: UpdateNoteFrame): ReactElement {
  return (
    <UpdateNoteShell {...frame}>
      <div className="flex items-center gap-2">
        <p className="min-w-0 flex-1 text-ink">
          Reload the page to see the update.
        </p>
        <button
          type="button"
          onClick={() => {
            location.reload();
          }}
          className={`rounded px-1 text-caption text-ink-3 hover:text-ink ${FOCUS_RING}`}
        >
          Reload
        </button>
      </div>
    </UpdateNoteShell>
  );
}
