/**
 * The small popover the update chip in the avatar menu opens (spec S28-S31):
 * which version this server runs, which one is available, the release notes,
 * the command to run on the server and where the check runs.
 *
 * Anchored to the avatar rather than to the chip: the chip is inside the menu,
 * which closes when the chip is chosen. There is no popover trigger, so a
 * close by Escape hands the focus back to the avatar by hand. A close by a
 * click or Tab elsewhere leaves the focus where the reader put it.
 *
 * Lazy, like the menu's body: only an admin with an update ever opens it.
 */

import { ExternalLink } from "lucide-react";
import { Popover } from "radix-ui";
import type { ReactElement, RefObject } from "react";

import { releaseNotesUrl, UPDATE_COMMAND } from "../auth/updateNote";
import { FOCUS_RING } from "./primitives";
import { copyText, useSaid } from "./useSaid";

export default function UpdateNote({
  anchor,
  current,
  available,
  onClose,
}: {
  /** The avatar: where the popover opens and where the focus goes back to. */
  anchor: RefObject<HTMLButtonElement | null>;
  /** The version this server runs. */
  current: string;
  /** The newer version, already checked to be a plain `x.y.z`. */
  available: string;
  onClose: () => void;
}): ReactElement {
  const [said, say] = useSaid();
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
          <p className="text-ink">
            This server runs {current}. {available} is available.
          </p>
          <a
            href={releaseNotesUrl(available)}
            target="_blank"
            rel="noopener noreferrer"
            className={`inline-flex items-center gap-1 rounded text-link underline-offset-2 hover:underline ${FOCUS_RING}`}
          >
            Release notes <span className="sr-only">(opens in a new tab)</span>
            <ExternalLink aria-hidden="true" size={13} strokeWidth={1.75} />
          </a>
          <div>
            <p>Run on the server:</p>
            <div className="mt-1 flex items-center gap-2">
              <code className="min-w-0 flex-1 truncate rounded bg-sunken px-1.5 py-0.5 font-mono text-[12.5px] text-ink">
                {UPDATE_COMMAND}
              </code>
              <button
                type="button"
                aria-label="Copy the command"
                onClick={() => {
                  copyText(UPDATE_COMMAND, say);
                }}
                className={`rounded px-1 text-caption text-ink-3 hover:text-ink ${FOCUS_RING}`}
              >
                Copy
              </button>
            </div>
            <span role="status" className="text-caption text-ink-3">
              {said}
            </span>
          </div>
          <p className="text-caption text-muted">
            The update check runs on the server.
          </p>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
