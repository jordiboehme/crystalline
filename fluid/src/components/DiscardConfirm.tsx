/**
 * The inline confirmation a discard asks for: one strip under the list (or
 * under a page's header) naming what goes, with the confirm and the way out
 * beside it. The two-step `DestructiveAction` idea drawn as a strip, because
 * it confirms a set rather than one control and sits under what it is
 * about; no typed name, because this loss names its files.
 */

import type { ReactElement } from "react";

import { BUTTON } from "./primitives";

const ALERT_CLASSES =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

export function DiscardConfirm({
  question,
  pending,
  problem,
  onConfirm,
  onCancel,
}: {
  /** The question, e.g. "Discard 3 files?" or "Discard notes/a.md?" */
  question: string;
  pending: boolean;
  /** A refusal to show in the strip, when the last attempt was refused. */
  problem: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}): ReactElement {
  return (
    <div
      role="group"
      aria-label={question}
      className="flex flex-col gap-2 rounded border border-red-300 p-2 dark:border-red-800"
      onKeyDown={(event) => {
        // Answered here and no further: the dialog this usually sits in reads
        // Escape as "leave", and somebody backing out of a discard is backing
        // out of the discard rather than out of the dialog.
        if (event.key === "Escape") {
          event.stopPropagation();
          onCancel();
        }
      }}
    >
      <p className="text-sm">{`${question} Their changes are put back the way the team has them.`}</p>
      {problem !== null && (
        <p role="alert" className={ALERT_CLASSES}>
          {problem}
        </p>
      )}
      <div className="flex flex-wrap justify-end gap-2">
        <button
          type="button"
          aria-label="Cancel discard"
          onClick={onCancel}
          className={BUTTON.secondary}
        >
          Cancel
        </button>
        <button
          type="button"
          autoFocus
          disabled={pending}
          onClick={onConfirm}
          className={BUTTON.destructive}
        >
          Confirm discard
        </button>
      </div>
    </div>
  );
}
