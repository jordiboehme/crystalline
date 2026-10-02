/**
 * The level select: the overlay the IDCLEV word opens on the game route.
 *
 * A text field over the domain list, in the HUD's own style (C19): white
 * monospace with the black shadow, on a dark panel over the dimmed
 * station. The list is `GET /domains` read under the sidebar's key
 * (`DOMAINS_QUERY_KEY`), which the room's own load has usually filled
 * already, so it shows at once; see `levels.ts` for the order, the filter
 * and the window.
 *
 * Typing goes into the field and filters the list. Up and down move the
 * selection, Enter or a click jumps (`onJump` with the domain's local
 * name, whichever name found it), and Esc closes (`onClose`). Those four
 * keys are read on `window`, as the CRT reader reads its keys, so they work
 * wherever the focus went; a key pressed with Ctrl, Cmd or Alt is left to
 * the browser (C18). A mousedown anywhere but on the field is prevented,
 * so a click never takes the focus from the field. The field swallows
 * auto-repeat until its first fresh key, so the V that completed the word,
 * if still held, types nothing (C17). While the listing loads, fails or
 * matches nothing, one line says so and Enter does nothing (C16).
 *
 * The field is a combobox over the listbox: `aria-expanded` while rows show,
 * `aria-controls` naming the list, and `aria-activedescendant` on the selected
 * option while it shows (absent when a status line stands in for the list).
 * Each option carries its place in the filtered list (`aria-setsize`,
 * `aria-posinset`), the status line is a `status`, and a key pressed during
 * an IME composition does nothing.
 *
 * The session already released the pointer lock and stopped reading keys
 * when it opened the select; the host unmounts it when the session says
 * the select closed.
 */

import { useQuery } from "@tanstack/react-query";
import { useId, useLayoutEffect, useMemo, useRef, useState } from "react";

import { DOMAINS_QUERY_KEY, fetchDomains } from "../../api/domains";
import { GAME_STALE_MS } from "../data/source";
import {
  LEVEL_ROWS,
  LEVELS_FAILED,
  LEVELS_FIELD,
  LEVELS_FOOTER,
  LEVELS_HERE,
  LEVELS_LABEL,
  LEVELS_LIST,
  LEVELS_LOADING,
  LEVELS_TITLE,
  NO_SUCH_LEVEL,
  filterLevels,
  hereKey,
  levelWindow,
  levelsOf,
  optionId,
  sortLevels,
  stepSelection,
} from "./levels";

/** The props of the level select. */
export interface LevelSelectProps {
  /**
   * The domain the player is in, by any name it answers to; its one row is
   * marked in the list (C14).
   */
  current: string;
  /** Enter or a click on a row: go to that domain's bridge. */
  onJump: (domain: string) => void;
  /** Esc: close the select. */
  onClose: () => void;
}

/** The overlay. See the module doc. */
export function LevelSelect({ current, onJump, onClose }: LevelSelectProps) {
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
    staleTime: GAME_STALE_MS,
  });
  const listId = useId();
  const footerId = useId();
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  // Whether the field has seen a fresh keydown yet (C17).
  const fresh = useRef(false);

  const sorted = useMemo(
    () => sortLevels(levelsOf(listing.data?.domains ?? []), (l) => l.label),
    [listing.data],
  );
  const shown = useMemo(
    () => filterLevels(sorted, query, (l) => l.terms),
    [sorted, query],
  );
  const here = useMemo(() => hereKey(sorted, current), [sorted, current]);
  const at = stepSelection(shown.length, selected, 0);
  const { start, end } = levelWindow(shown.length, at);

  // Layout effects: the focus and the keys are in place in the commit
  // that shows the select, so a fast first key after it opens is not lost.
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
          const level = shown[at];
          if (!event.repeat && level !== undefined) onJump(level.key);
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
  }, [shown, at, onJump, onClose]);

  const status =
    listing.data === undefined
      ? listing.isError
        ? LEVELS_FAILED
        : LEVELS_LOADING
      : shown.length === 0
        ? NO_SUCH_LEVEL
        : null;
  const position =
    shown.length === 0 ? "0/0" : `${String(at + 1)}/${String(shown.length)}`;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={LEVELS_LABEL}
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 px-4 pt-[15vh] font-mono text-white/85 [text-shadow:0_1px_2px_black]"
      onMouseDown={(event) => {
        if (event.target !== field.current) event.preventDefault();
      }}
    >
      <div className="w-full max-w-lg border border-white/30 bg-black/70 p-3 text-sm">
        <div className="flex justify-between gap-4 tracking-wider">
          <span>{LEVELS_TITLE}</span>
          <span aria-hidden="true">{position}</span>
        </div>
        <input
          ref={field}
          type="text"
          value={query}
          aria-label={LEVELS_FIELD}
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
            <ul id={listId} role="listbox" aria-label={LEVELS_LIST}>
              {shown.slice(start, end).map((level, n) => {
                const on = start + n === at;
                return (
                  <li
                    id={optionId(listId, start + n)}
                    aria-setsize={shown.length}
                    aria-posinset={start + n + 1}
                    key={level.key}
                    role="option"
                    aria-selected={on}
                    className={`flex h-6 cursor-pointer items-center justify-between gap-4 px-1 ${
                      on ? "bg-white text-black [text-shadow:none]" : ""
                    }`}
                    onClick={() => {
                      onJump(level.key);
                    }}
                  >
                    <span className="min-w-0 truncate">{level.label}</span>
                    {level.key === here && (
                      <span className="shrink-0 opacity-70">{LEVELS_HERE}</span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
        <div
          id={footerId}
          className="mt-2 whitespace-pre text-xs text-white/60"
        >
          {LEVELS_FOOTER}
        </div>
      </div>
    </div>
  );
}
