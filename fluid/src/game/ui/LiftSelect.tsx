/**
 * The lift overlay: the panel Space opens at a lift (M3 C26), over the
 * dimmed station in the HUD's own style (C19), same as the level select.
 *
 * Unlike the level select, the stops are not fetched: the session already
 * read them off the lift and hands them over with `onLift`, so the host
 * passes them straight through as props. The list keeps the lift's own
 * order and is never re-sorted; see `lift.ts` for the rows, the filter and
 * the window. A private stop is drawn with a small inline key before its
 * label, with the accessible text `LIFT_PRIVATE` for anything that reads
 * the page rather than looks at it.
 *
 * Typing goes into the field and filters the list. Up and down move the
 * selection, Enter or a click rides (`onRide` with the stop's own index in
 * the lift's `stops` array, whatever the filter currently shows), and Esc
 * closes (`onClose`). Those four keys are read on `window`, as the level
 * select's are, so they work wherever the focus went; a key pressed with
 * Ctrl, Cmd or Alt is left to the browser. A mousedown anywhere but on the
 * field is prevented, so a click never takes the focus from the field. The
 * field swallows auto-repeat until its first fresh key, so a key still held
 * from opening the overlay types nothing. While the filter matches nothing,
 * one line says so and Enter does nothing; a lift with no stops at all
 * (never a filter's doing) says so with its own distinct line, `NO STOPS`,
 * so an empty listing never misreads as a filter miss.
 *
 * The field is a combobox over the listbox: `aria-expanded` while rows show,
 * `aria-controls` naming the list, and `aria-activedescendant` on the selected
 * option while it shows (absent when a status line stands in for the list).
 * Each option carries its place in the filtered list (`aria-setsize`,
 * `aria-posinset`), the status line is a `status`, and a key pressed during
 * an IME composition does nothing.
 *
 * The session already released the pointer lock and stopped reading keys
 * when it opened the overlay; the host unmounts it when the session says
 * the overlay closed.
 */

import { useId, useLayoutEffect, useMemo, useRef, useState } from "react";

import type { LiftStop } from "../world/types";
import {
  LIFT_FIELD,
  LIFT_FOOTER,
  LIFT_HERE,
  LIFT_LABEL,
  LIFT_LIST,
  LIFT_PRIVATE,
  LIFT_TITLE,
  NO_STOPS,
  NO_SUCH_STOP,
  liftRows,
} from "./lift";
import {
  LEVEL_ROWS,
  filterLevels,
  levelWindow,
  optionId,
  stepSelection,
} from "./levels";

/** The props of the lift overlay. */
export interface LiftSelectProps {
  /** The lift's stops, in the lift's own order. */
  stops: LiftStop[];
  /** A status line shown under the list, or null. */
  note: string | null;
  /** Enter or a click on a row: ride to that stop's own index in `stops`. */
  onRide: (stop: number) => void;
  /** Esc: close the overlay. */
  onClose: () => void;
}

/** The key pictogram drawn before a private stop's label. */
function KeyGlyph() {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      width={12}
      height={12}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.4}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="5" cy="5" r="3.1" />
      <path d="M7.2 7.2L13.5 13.5M11 11l1.5-1.5M13 13l1.3-1.3" />
    </svg>
  );
}

/** The overlay. See the module doc. */
export function LiftSelect({ stops, note, onRide, onClose }: LiftSelectProps) {
  const listId = useId();
  const footerId = useId();
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  // Whether the field has seen a fresh keydown yet.
  const fresh = useRef(false);

  const rows = useMemo(() => liftRows(stops), [stops]);
  const shown = useMemo(
    () => filterLevels(rows, query, (r) => [r.label]),
    [rows, query],
  );
  const at = stepSelection(shown.length, selected, 0);
  const { start, end } = levelWindow(shown.length, at);

  // Layout effects: the focus and the keys are in place in the commit
  // that shows the overlay, so a fast first key after Space is not lost.
  useLayoutEffect(() => {
    field.current?.focus();
  }, []);

  useLayoutEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // An IME composition owns Enter and the arrows. Safari sends the
      // composition's final Enter with `isComposing` false and keyCode 229.
      if (event.isComposing || event.keyCode === 229) return;
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      switch (event.key) {
        case "ArrowUp":
          setSelected(stepSelection(shown.length, at, -1));
          break;
        case "ArrowDown":
          setSelected(stepSelection(shown.length, at, 1));
          break;
        case "Enter": {
          const row = shown[at];
          if (!event.repeat && row !== undefined) onRide(row.index);
          break;
        }
        case "Escape":
          if (!event.repeat) onClose();
          break;
        default:
          return;
      }
      event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  }, [shown, at, onRide, onClose]);

  const status =
    stops.length === 0 ? NO_STOPS : shown.length === 0 ? NO_SUCH_STOP : null;
  const position =
    shown.length === 0 ? "0/0" : `${String(at + 1)}/${String(shown.length)}`;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={LIFT_LABEL}
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 px-4 pt-[15vh] font-mono text-white/85 [text-shadow:0_1px_2px_black]"
      onMouseDown={(event) => {
        if (event.target !== field.current) event.preventDefault();
      }}
    >
      <div className="w-full max-w-lg border border-white/30 bg-black/70 p-3 text-sm">
        <div className="flex justify-between gap-4 tracking-wider">
          <span>{LIFT_TITLE}</span>
          <span aria-hidden="true">{position}</span>
        </div>
        <input
          ref={field}
          type="text"
          value={query}
          aria-label={LIFT_FIELD}
          role="combobox"
          aria-expanded={status === null}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-describedby={footerId}
          {...(status === null
            ? { "aria-activedescendant": optionId(listId, at) }
            : {})}
          autoComplete="off"
          spellCheck={false}
          className="mt-2 w-full border-b border-white/40 bg-transparent py-1 text-base text-white outline-none"
          onChange={(event) => {
            setQuery(event.target.value);
            setSelected(0);
          }}
          onKeyDown={(event) => {
            if (!event.repeat) {
              fresh.current = true;
            } else if (!fresh.current) {
              event.preventDefault();
            }
          }}
        />
        <div
          className="mt-2"
          style={{ minHeight: `${String(LEVEL_ROWS * 1.5)}rem` }}
        >
          <p role="status" className={status !== null ? "py-1" : undefined}>
            {status ?? ""}
          </p>
          {status === null && (
            <ul id={listId} role="listbox" aria-label={LIFT_LIST}>
              {shown.slice(start, end).map((row, n) => {
                const on = start + n === at;
                return (
                  <li
                    id={optionId(listId, start + n)}
                    aria-setsize={shown.length}
                    aria-posinset={start + n + 1}
                    key={row.index}
                    role="option"
                    aria-selected={on}
                    className={`flex h-6 cursor-pointer items-center justify-between gap-4 px-1 ${
                      on ? "bg-white text-black [text-shadow:none]" : ""
                    }`}
                    onClick={() => {
                      onRide(row.index);
                    }}
                  >
                    <span className="flex min-w-0 items-center gap-1">
                      {row.key && (
                        <span
                          role="img"
                          aria-label={LIFT_PRIVATE}
                          className="shrink-0"
                        >
                          <KeyGlyph />
                        </span>
                      )}
                      <span className="min-w-0 truncate">{row.label}</span>
                    </span>
                    {row.here && (
                      <span className="shrink-0 opacity-70">{LIFT_HERE}</span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
        {note !== null && (
          <div role="status" className="mt-2 opacity-70">
            {note}
          </div>
        )}
        <div
          id={footerId}
          className="mt-2 whitespace-pre text-xs text-white/60"
        >
          {LIFT_FOOTER}
        </div>
      </div>
    </div>
  );
}
