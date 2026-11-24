/**
 * The end of a printed Reflection page: its open threads as numbered Notes.
 * Its own module, read by the Reflection page only. Like every print-only
 * block it carries `hidden` and `data-print` (see `PrintSheet.tsx`).
 */

import type { ReactElement } from "react";

import { formatDayLong } from "../format";
import type { PrintNote } from "../printNotes";

/** What a thread entry says, as the thread card words it. */
function entryText(entry: PrintNote["thread"]["entries"][number]): string {
  if (entry.kind === "resolve") {
    return entry.text === "" ? "Resolved" : `Resolved: ${entry.text}`;
  }
  if (entry.kind === "reopen") {
    return entry.text === "" ? "Reopened" : `Reopened: ${entry.text}`;
  }
  return entry.text;
}

/** Where a note sits, in a few words. */
function placeOf(note: PrintNote): string | null {
  if (note.state === "section_gone") {
    return "Its section is gone.";
  }
  if (note.state === "text_changed") {
    return note.heading === null
      ? "The text changed."
      : `${note.heading}. The text changed.`;
  }
  if (note.quote === null) {
    return note.heading === null
      ? "On the whole intro."
      : `On the whole section ${note.heading}.`;
  }
  return note.heading;
}

/**
 * The open threads of a Reflection page, as numbered notes at its end. The
 * same numbers stand at the end of each quote in the printed text.
 */
export function PrintNotes({
  notes,
}: {
  notes: readonly PrintNote[];
}): ReactElement | null {
  if (notes.length === 0) {
    return null;
  }
  return (
    <section
      hidden
      data-print=""
      aria-label="Notes"
      className="print-notes mt-10 border-t border-hair-strong pt-3"
    >
      <h2 className="text-prose-h3">Notes</h2>
      <p className="mb-3 text-caption text-muted">
        The open threads on this page. The numbers match the marks in the text.
      </p>
      <ol className="flex flex-col gap-4">
        {notes.map((note) => {
          const place = placeOf(note);
          return (
            <li
              key={note.thread.id}
              className="print-note grid grid-cols-[1.75rem_minmax(0,1fr)] text-sm"
            >
              <span className="font-semibold text-ink tabular-nums">
                {note.n}
              </span>
              <div className="flex flex-col gap-1.5">
                {note.quote !== null && (
                  <blockquote className="border-l-2 border-(--mark-rule) pl-2.5 text-ink-3 italic">
                    {note.quote}
                  </blockquote>
                )}
                {place !== null && (
                  <p className="text-caption text-muted">{place}</p>
                )}
                {note.thread.entries.map((entry, index) => (
                  <div key={`${entry.at}-${String(index)}`}>
                    <p className="text-caption">
                      <span className="font-semibold text-ink">
                        {entry.author.name}
                        {entry.author.agent ? " (agent)" : ""}
                      </span>
                      <span className="text-muted">{` · ${formatDayLong(entry.at)}`}</span>
                    </p>
                    <p className="whitespace-pre-wrap text-ink-2">
                      {entryText(entry)}
                    </p>
                  </div>
                ))}
              </div>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
