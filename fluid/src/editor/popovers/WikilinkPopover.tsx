/**
 * The "Wiki link" popover, on engram pages only: the target between `[[` and
 * `]]` (the part before `|` when an alias is written), with the title
 * matches the `[[` completion offers. Save or Enter writes only the target;
 * "Remove link" takes the brackets (and `Target|`) away and keeps the words
 * a reader saw; "Open" goes to the engram the chip names, in the app.
 */

import type { KeyboardEvent, ReactElement } from "react";
import { useEffect, useId, useState } from "react";
import { useNavigate } from "react-router";

import { BUTTON, FIELD } from "../../components/primitives";
import {
  unwikilinkChanges,
  wikilinkAt,
  wikilinkChanges,
} from "../elementVerbs";
import type { TitleOption } from "../wikilinkChips";
import { wikilinkResolverFacet } from "../wikilinkChips";
import type { PopoverBodyProps } from "./ElementPopover";

/** How long typing rests before the titles are asked for, in ms. */
const ASK_AFTER_MS = 150;

function validTarget(target: string): boolean {
  const trimmed = target.trim();
  return trimmed !== "" && !/[[\]|\r\n]/.test(trimmed);
}

export function WikilinkPopover({
  view,
  edit,
  titles,
  commit,
  close,
  live,
}: PopoverBodyProps): ReactElement | null {
  const id = useId();
  const navigate = useNavigate();
  /** The route the wikilink at `at` names in the document now, or null. */
  const hrefOf = (at: number): string | null => {
    const link = wikilinkAt(view.state, at);
    if (!link) return null;
    const inner = view.state.doc.sliceString(link.from + 2, link.to - 2);
    const resolution = view.state.facet(wikilinkResolverFacet)(inner);
    return resolution?.kind === "resolved" ? resolution.href : null;
  };
  const [initial] = useState(() => {
    if (edit.kind !== "wikilink") return { target: "", href: null };
    const link = wikilinkAt(view.state, edit.at);
    if (!link) return { target: "", href: null };
    return {
      target: view.state.doc.sliceString(link.target.from, link.target.to),
      href: hrefOf(edit.at),
    };
  });
  const [value, setValue] = useState(initial.target);
  const [typed, setTyped] = useState(false);
  const [options, setOptions] = useState<TitleOption[]>([]);
  const [listOpen, setListOpen] = useState(false);
  const [active, setActive] = useState(-1);

  // The title matches for what is typed, once typing rests.
  useEffect(() => {
    if (!typed || titles === undefined || value.trim() === "") return;
    let live = true;
    const timer = setTimeout(() => {
      void titles(value.trim())
        .then((found) => {
          if (!live) return;
          setOptions(found);
          setActive(-1);
          setListOpen(found.length > 0);
        })
        .catch(() => {
          // No suggestions; the field still takes what is typed.
        });
    }, ASK_AFTER_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [typed, titles, value]);

  if (edit.kind !== "wikilink") return null;
  const valid = validTarget(value);
  const listId = `${id}-titles`;

  const save = () => {
    if (!valid) return;
    // An untouched target keeps whatever the document holds now.
    if (value === initial.target) {
      commit(() => []);
      return;
    }
    commit((state, now) =>
      now.kind === "wikilink"
        ? wikilinkChanges(state, now.at, { target: value })
        : null,
    );
  };
  const pick = (option: TitleOption) => {
    setValue(option.name);
    setListOpen(false);
    setActive(-1);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    const shown = listOpen && options.length > 0;
    if (event.key === "ArrowDown" && options.length > 0) {
      event.preventDefault();
      setListOpen(true);
      setActive((now) => (now + 1) % options.length);
    } else if (event.key === "ArrowUp" && shown) {
      event.preventDefault();
      setActive((now) => (now <= 0 ? options.length - 1 : now - 1));
    } else if (event.key === "Enter" && shown && active >= 0) {
      event.preventDefault();
      const option = options[active];
      if (option) pick(option);
    } else if (event.key === "Escape" && shown) {
      // The first Escape closes the suggestions only.
      event.preventDefault();
      event.stopPropagation();
      setListOpen(false);
      setActive(-1);
    }
  };

  const href = initial.href;
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        save();
      }}
    >
      <div className="relative flex flex-col gap-1">
        <label htmlFor={`${id}-target`}>Target</label>
        <input
          id={`${id}-target`}
          className={FIELD}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={listOpen && options.length > 0}
          aria-controls={listId}
          {...(listOpen && active >= 0
            ? { "aria-activedescendant": `${listId}-${String(active)}` }
            : {})}
          value={value}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => {
            setValue(event.target.value);
            setTyped(true);
          }}
          onKeyDown={onKeyDown}
        />
        {listOpen && options.length > 0 && (
          <ul
            id={listId}
            role="listbox"
            aria-label="Titles"
            className="menu-face absolute top-full right-0 left-0 z-10 mt-1 max-h-56 overflow-y-auto"
          >
            {options.map((option, index) => (
              <li
                key={option.name}
                id={`${listId}-${String(index)}`}
                role="option"
                aria-selected={index === active}
                className={`menu-row ${index === active ? "bg-slate-100 dark:bg-slate-800" : ""}`}
                onMouseDown={(event) => {
                  event.preventDefault();
                  pick(option);
                }}
              >
                {option.label}
                {option.detail !== undefined && (
                  <span className="ml-2 text-slate-500 dark:text-slate-400">
                    {option.detail}
                  </span>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button type="submit" className={BUTTON.primary} disabled={!valid}>
          Save
        </button>
        <button
          type="button"
          className={BUTTON.secondary}
          disabled={href === null}
          onClick={() => {
            // The engram the chip names now: a peer may have changed it.
            const now = live();
            const target = now?.kind === "wikilink" ? hrefOf(now.at) : null;
            if (target === null) return;
            close();
            void navigate(target);
          }}
        >
          Open
        </button>
        <button
          type="button"
          className={BUTTON.ghost}
          onClick={() => {
            commit((state, now) =>
              now.kind === "wikilink" ? unwikilinkChanges(state, now.at) : null,
            );
          }}
        >
          Remove link
        </button>
      </div>
    </form>
  );
}
