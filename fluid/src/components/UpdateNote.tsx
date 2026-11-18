/**
 * The update note the chip in the avatar menu opens (spec S28-S31): which
 * version this server runs, which one is available, the release notes, the
 * command to run on the server and where the check runs. The popover frame
 * and its focus rules are in `UpdateNoteShell.tsx`.
 *
 * Lazy, like the menu's body: only an admin with an update ever opens it.
 */

import { ExternalLink } from "lucide-react";
import type { ReactElement } from "react";

import { releaseNotesUrl, UPDATE_COMMAND } from "../auth/updateNote";
import { FOCUS_RING } from "./primitives";
import { UpdateNoteShell } from "./UpdateNoteShell";
import type { UpdateNoteFrame } from "./UpdateNoteShell";
import { copyText, useSaid } from "./useSaid";

export default function UpdateNote({
  current,
  ...frame
}: UpdateNoteFrame & {
  /** The version this server runs. */
  current: string;
}): ReactElement {
  const { available } = frame;
  const [said, say] = useSaid();
  return (
    <UpdateNoteShell {...frame}>
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
    </UpdateNoteShell>
  );
}
