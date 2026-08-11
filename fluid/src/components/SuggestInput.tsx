/**
 * A text field that shows the words it recommends instead of expecting them to
 * be remembered.
 *
 * `type` and `status` are free form by design, and a native datalist made that
 * design unusable in the one place it matters: the list only appears once the
 * first characters happen to match, it cannot say what any of the words mean,
 * and a field opened on a value it already holds shows nothing at all. So this
 * is a combobox with the list as the point of it - focus opens the whole
 * vocabulary, each recommended word with a line saying what it is for, and
 * typing narrows it.
 *
 * What it deliberately does NOT do is enforce. Anything can be typed, an
 * unlisted value is written exactly as entered, and nothing here marks it as
 * wrong: the frontmatter fields these serve are free form because a domain is
 * allowed words this app never heard of, and a control that quietly refused
 * them would be lying about the format.
 *
 * That promise is what decides the keyboard. Nothing in the list is active
 * until an arrow key makes it so, and Enter on a field with no active row
 * accepts what was typed rather than the nearest word on offer. The other
 * variant - a row active as soon as the list opens - is for a field whose
 * value is expected to come from the list, and taking it here would mean a
 * house word that happens to contain a recommended one could not be entered
 * with the key most people finish a field with.
 *
 * Hand-rolled rather than built on the palette's `cmdk` or on a Radix popover:
 * both own a keyboard that fights a text input (a menu takes the arrow keys
 * and the typeahead for itself), and `cmdk` hard-codes `aria-expanded="true"`
 * on its input, which is a lie in the closed state a field spends most of its
 * life in. The surface is drawn with the shared menu classes anyway, so it is
 * the same floating panel a reader has already met elsewhere in the app.
 */

import type { ReactElement } from "react";
import { useEffect, useRef, useState } from "react";

import { MENU_CLASSES } from "./menu";

/** One offered value: the word, what it is for, and how used it is. */
export interface Suggestion {
  /** The value itself, written into the field verbatim when it is picked. */
  name: string;
  /** One line on what the word is for. Absent for a value in use. */
  gloss?: string | undefined;
  /** How many engrams already carry it, when the source knows. */
  count?: number | undefined;
}

export interface SuggestInputProps {
  /** The field's id, which its label points at. */
  id: string;
  /**
   * What the field is called, exactly as its label says it. The list is named
   * from it ("Status suggestions"), which is a name of its own rather than a
   * second element answering to "Status".
   */
  label: string;
  /** The value the field opens on. */
  value: string;
  /** What it offers, in the order it should read. */
  suggestions: readonly Suggestion[];
  /** Every keystroke, for a caller holding the value in state. */
  onChange?: ((next: string) => void) | undefined;
  /** The settled value: a pick, or a blur. Never fired for an unchanged one. */
  onCommit?: ((next: string) => void) | undefined;
  /** Help text elsewhere on the screen, for `aria-describedby`. */
  describedBy?: string | undefined;
  /** The field's own look, which each screen already owns. */
  className?: string | undefined;
  placeholder?: string | undefined;
}

/** No row is active. The position before the first, so `index + 1` is first. */
const NO_ROW = -1;

/**
 * One row's two faces, each a whole class string.
 *
 * Whole strings rather than a highlight layered onto a base, for the reason
 * `TOGGLE` in the primitives spells out at length: Tailwind resolves
 * same-specificity conflicts by the order utilities are emitted into the
 * stylesheet, not by the order of names in a class attribute, so a background
 * appended to a base class could silently lose to it. The pair is the one the
 * command palette's rows already use, so a highlighted row means the same
 * thing on both of the app's floating lists.
 */
const OPTION = {
  off: "flex cursor-pointer items-baseline gap-2 rounded px-2 py-1.5 text-sm text-slate-700 dark:text-slate-300",
  on: "flex cursor-pointer items-baseline gap-2 rounded bg-slate-100 px-2 py-1.5 text-sm text-slate-700 dark:bg-slate-800 dark:text-slate-300",
} as const;

/**
 * Whether the Escape being handled belongs to an open suggestion list.
 *
 * A dialog that dismisses on Escape asks this before dismissing, because the
 * two dismissals cannot sort themselves out by ordinary event propagation: a
 * dismissable layer listens on the document in the CAPTURE phase, so it has
 * already decided by the time the key reaches the field somebody is typing in,
 * and no `stopPropagation` in here can get ahead of it. The field cannot win
 * the race, so the layer asks instead - and it asks the DOM, which is the same
 * state the person pressing the key can see, rather than a flag two components
 * would have to keep in step. Every dismissable layer that can contain one of
 * these fields must ask this before it dismisses, or Escape will close the
 * layer instead of the list every time.
 *
 * Exported from the component's own file rather than from a module of its own,
 * for the reason the primitives make the same exception: the knowledge is
 * about this control, and a helper file holding one predicate would only make
 * it easier to change one of them without the other.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function suggestionsAreOpen(): boolean {
  return (
    document.activeElement?.matches(
      '[role="combobox"][aria-expanded="true"]',
    ) === true
  );
}

export function SuggestInput({
  id,
  label,
  value,
  suggestions,
  onChange,
  onCommit,
  describedBy,
  className,
  placeholder,
}: SuggestInputProps): ReactElement {
  const [draft, setDraft] = useState(value);
  const [open, setOpen] = useState(false);
  // Whether the draft is being used as a filter. Opening the field is not
  // typing: a field opened on `stable` shows the whole vocabulary, and only a
  // keystroke narrows it. This is the difference between a list worth opening
  // and the datalist behavior this control replaces.
  const [filtering, setFiltering] = useState(false);
  // Which row is active, or NO_ROW for none.
  //
  // Nothing is active until an arrow key asks for something, which is the
  // whole difference between the two combobox variants and the difference
  // between guidance and enforcement here. With a row active from the moment
  // the list opens, Enter takes it: typing `e` on the way to `experimental`
  // would make `stable` active and Enter would write `stable` over what was
  // typed. That variant is for fields whose value is expected to come from
  // the list, and these two fields are the opposite of that by design.
  const [active, setActive] = useState(NO_ROW);
  const focused = useRef(false);
  // What the owner last heard, so a pick followed by a blur is one commit and
  // a blur that changed nothing is none. In the frontmatter rail a commit is a
  // document transaction, and a redundant one would be an undo step that
  // undoes nothing.
  const committed = useRef(value);

  // The owner's value is the truth whenever this field is not being typed in:
  // a hand edit in the buffer behind the rail reaches the field this way. The
  // guard is what keeps it from fighting the person typing - and after a pick,
  // the value coming back is the one just written, so nothing moves.
  useEffect(() => {
    if (!focused.current) {
      setDraft(value);
      committed.current = value;
    }
  }, [value]);

  const needle = draft.trim().toLowerCase();
  const matches =
    filtering && needle !== ""
      ? suggestions.filter((suggestion) =>
          suggestion.name.toLowerCase().includes(needle),
        )
      : [...suggestions];
  // A list with nothing in it is not an open list, and `aria-expanded` must
  // not claim otherwise: a typed value nobody recommends simply has no popover.
  const expanded = open && matches.length > 0;
  // Clamped against a list that may have shrunk under it, and never below
  // NO_ROW, so an active row is always either a row that exists or none.
  const index =
    active === NO_ROW ? NO_ROW : Math.min(active, matches.length - 1);
  const hasActive = expanded && index >= 0;
  const listId = `${id}-suggestions`;
  const optionId = (position: number) => `${id}-suggestion-${String(position)}`;

  const change = (next: string) => {
    setDraft(next);
    setFiltering(true);
    // Typing narrows the list; it never picks from it. What is in the field is
    // what the person wrote until they say otherwise with an arrow key.
    setActive(NO_ROW);
    setOpen(true);
    onChange?.(next);
  };

  const commit = (next: string) => {
    if (next === committed.current) {
      return;
    }
    committed.current = next;
    onCommit?.(next);
  };

  const pick = (suggestion: Suggestion) => {
    setDraft(suggestion.name);
    setFiltering(false);
    setOpen(false);
    onChange?.(suggestion.name);
    commit(suggestion.name);
  };

  return (
    <div className="relative">
      <input
        id={id}
        role="combobox"
        aria-expanded={expanded}
        aria-controls={listId}
        aria-autocomplete="list"
        {...(hasActive ? { "aria-activedescendant": optionId(index) } : {})}
        {...(describedBy !== undefined
          ? { "aria-describedby": describedBy }
          : {})}
        {...(placeholder !== undefined ? { placeholder } : {})}
        className={className ?? ""}
        value={draft}
        onChange={(event) => {
          change(event.target.value);
        }}
        onFocus={() => {
          focused.current = true;
          setFiltering(false);
          setActive(NO_ROW);
          setOpen(true);
        }}
        onClick={() => {
          // Opens, never toggles: a click on a field somebody is already in is
          // a click into the text, not a request to put the list away.
          setOpen(true);
        }}
        onBlur={() => {
          // The options never take the focus (their mousedown is prevented),
          // so losing it means leaving the control: put the list away and let
          // whatever is in the field stand, recommended or not.
          focused.current = false;
          setOpen(false);
          setFiltering(false);
          commit(draft);
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            if (!open) {
              setFiltering(false);
              setActive(NO_ROW);
              setOpen(true);
              return;
            }
            // From no row, down is the first one; `index + 1` says exactly
            // that, since NO_ROW is the position before the first.
            setActive(Math.max(0, Math.min(index + 1, matches.length - 1)));
            return;
          }
          if (event.key === "ArrowUp") {
            event.preventDefault();
            if (!open) {
              setFiltering(false);
              setActive(NO_ROW);
              setOpen(true);
              return;
            }
            // From no row, up is the last one, which is what a person
            // reaching upwards into a list beneath the field means.
            setActive(
              index === NO_ROW ? matches.length - 1 : Math.max(index - 1, 0),
            );
            return;
          }
          if (event.key === "Enter") {
            const chosen = hasActive ? matches[index] : undefined;
            if (chosen) {
              // A row was walked to, so Enter takes it - and only then is the
              // key this control's to keep.
              event.preventDefault();
              pick(chosen);
              return;
            }
            // Nothing was walked to, so what is in the field is what was
            // typed, and Enter accepts it: the list goes away and the value
            // is committed as written. The event is DELIBERATELY not stopped,
            // so a form around the field submits on this same keypress the way
            // it did when these fields were plain inputs with a datalist. One
            // Enter, one submit, and the value that submits is the typed one.
            setOpen(false);
            setFiltering(false);
            commit(draft);
            return;
          }
          if (event.key === "Escape" && open) {
            // Stopped rather than merely handled: this field lives inside a
            // dialog that also closes on Escape, and dismissing a list must
            // not dismiss the form around it.
            event.preventDefault();
            event.stopPropagation();
            setOpen(false);
            setFiltering(false);
          }
        }}
      />
      {expanded && (
        <ul
          id={listId}
          role="listbox"
          // Named for what it is, in the field's own words: a listbox must
          // have an accessible name, and this one must not be the field's own
          // name. Pointing it back at the label would make a second element on
          // the page answer to "Status", which is exactly how a screen reader
          // user and every test in this suite find the field itself.
          aria-label={`${label} suggestions`}
          className={`absolute top-full left-0 mt-1 max-h-64 w-full overflow-y-auto ${MENU_CLASSES}`}
        >
          {matches.map((suggestion, position) => (
            <li
              key={suggestion.name}
              id={optionId(position)}
              role="option"
              aria-selected={hasActive && position === index}
              data-value={suggestion.name}
              className={
                hasActive && position === index ? OPTION.on : OPTION.off
              }
              onMouseDown={(event) => {
                // The field keeps the focus through the whole pick: a blur
                // here would close the list out from under the click.
                event.preventDefault();
              }}
              onMouseEnter={() => {
                setActive(position);
              }}
              onClick={() => {
                pick(suggestion);
              }}
            >
              <span className="font-medium">{suggestion.name}</span>
              {/*
                One step darker than the caption gray the rest of the app uses
                on a plain surface, because these captions sit on the highlight
                wash as well: slate-500 on slate-100 is 4.35:1, under the 4.5
                floor for 12px text, while slate-600 clears it on both grounds
                (7.56 on white, 6.90 on the wash). The dark pair needs no such
                move (slate-400 is 6.79 on slate-900 and 5.58 on slate-800).
              */}
              {suggestion.gloss !== undefined && (
                <span className="text-caption text-slate-600 dark:text-slate-400">
                  {suggestion.gloss}
                </span>
              )}
              {suggestion.count !== undefined && (
                <span className="ml-auto text-caption tabular-nums text-slate-600 dark:text-slate-400">
                  {suggestion.count}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
