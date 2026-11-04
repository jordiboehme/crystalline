/**
 * The code box's menus.
 *
 * "Code language": a filter field and every language the editor knows after
 * "Plain text", the box's language checked. Typing only filters; picking a
 * language (a click, or Enter on it or in the field for the first match)
 * writes only the info word (`languageChanges`), which is re-read at commit,
 * so a word a peer changed meanwhile is the one replaced. Picking the
 * language the word names at commit writes nothing (`typescript` stays when
 * TypeScript is picked), and so does Plain text on a word no language has.
 *
 * "Code block actions": the language menu again (the keyboard's way to it),
 * and "Add a paragraph below" (`paragraphBelowChanges`), the way out of a
 * box that ends the document.
 */

import type { KeyboardEvent, ReactElement } from "react";
import { useEffect, useRef, useState } from "react";

import { ITEM_CLASSES } from "../../components/menu";
import type { LanguageChoice } from "../codeBlocks";
import { LANGUAGE_CHOICES, languageName } from "../codeBlocks";
import { editAt, requestElementEdit } from "../elementEdit";
import {
  codeFenceAt,
  languageChanges,
  languageOf,
  paragraphBelowChanges,
} from "../elementVerbs";
import type { ElementWrite, PopoverBodyProps } from "./ElementPopover";
import { RadioItem } from "./TaskMenu";

/** Whether `choice` matches what is typed in the filter field. */
function matches(choice: LanguageChoice, typed: string): boolean {
  const term = typed.trim().toLowerCase();
  return (
    term === "" ||
    choice.name.toLowerCase().includes(term) ||
    (choice.word?.includes(term) ?? false)
  );
}

/**
 * What picking `choice` writes, decided on the box as it is at commit: nothing
 * when its word already names that language (or names none and Plain text is
 * picked), else the choice's word over the word.
 */
function pickWrite(choice: LanguageChoice): ElementWrite {
  return (state, now) => {
    if (now.kind !== "language") return null;
    const fence = codeFenceAt(state, now.at);
    if (fence?.from !== now.at) return null;
    const written = languageOf(
      state.doc.sliceString(fence.word.from, fence.word.to),
    );
    const asked = languageOf(choice.word);
    if (asked === written) return [];
    return languageChanges(state, now.at, choice.word);
  };
}

export function LanguageMenu({
  view,
  edit,
  commit,
}: PopoverBodyProps): ReactElement {
  const [current] = useState(() => {
    const fence = codeFenceAt(view.state, editAt(edit));
    return languageName(
      fence ? view.state.doc.sliceString(fence.word.from, fence.word.to) : null,
    );
  });
  const [typed, setTyped] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const shown = LANGUAGE_CHOICES.filter((choice) => matches(choice, typed));

  // The checked language in sight when the menu opens.
  useEffect(() => {
    const checked = list.current?.querySelector<HTMLElement>(
      '[aria-checked="true"]',
    );
    // Not every browser (or jsdom) has it.
    if (typeof checked?.scrollIntoView === "function") {
      checked.scrollIntoView({ block: "nearest" });
    }
  }, []);

  const [refused, setRefused] = useState(false);
  const pick = (choice: LanguageChoice) => {
    // Plain text cannot leave more info behind: CommonMark would read that
    // text as the language. The word stays, and the menu says why.
    if (choice.word === null) {
      const fence = codeFenceAt(view.state, editAt(edit));
      const written = fence
        ? languageOf(view.state.doc.sliceString(fence.word.from, fence.word.to))
        : null;
      if (fence?.more === true && written !== null) {
        setRefused(true);
        return;
      }
    }
    commit(pickWrite(choice));
  };
  const onFieldKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    // An empty field picks the checked language; a filter its first match,
    // and nothing when nothing matches.
    const choice =
      typed.trim() === ""
        ? LANGUAGE_CHOICES.find((entry) => entry.name === current)
        : shown[0];
    if (choice) pick(choice);
  };
  return (
    <>
      <div className="px-2 pb-1">
        <input
          type="text"
          aria-label="Filter languages"
          placeholder="Filter languages"
          data-menu-filter=""
          value={typed}
          onChange={(event) => {
            setTyped(event.target.value);
          }}
          onKeyDown={onFieldKey}
          className="w-full rounded border border-slate-200 bg-white px-2 py-1 text-sm dark:border-slate-700 dark:bg-slate-900"
        />
      </div>
      <div
        ref={list}
        role="group"
        aria-label="Languages"
        className="max-h-64 overflow-y-auto"
      >
        {shown.map((choice) => (
          <RadioItem
            key={choice.name}
            name={choice.name}
            checked={choice.name === current}
            onPick={() => {
              pick(choice);
            }}
          />
        ))}
      </div>
      {refused ? (
        <p
          role="status"
          className="px-3 pt-1 text-xs text-slate-600 dark:text-slate-300"
        >
          Plain text cannot keep the info after the language.
        </p>
      ) : null}
    </>
  );
}

/** One action of a menu. */
function ActionItem({
  name,
  onPick,
}: {
  name: string;
  onPick: () => void;
}): ReactElement {
  return (
    <button
      type="button"
      role="menuitem"
      tabIndex={-1}
      className={`${ITEM_CLASSES} w-full text-left`}
      onClick={onPick}
    >
      {name}
    </button>
  );
}

export function CodeActionsMenu({
  view,
  edit,
  commit,
  close,
  live,
}: PopoverBodyProps): ReactElement {
  const toLanguage = () => {
    const now = live();
    close();
    // The language menu in its place, for the same box as it is now.
    if (now?.kind === "code-actions") {
      requestElementEdit(view, {
        kind: "language",
        at: now.at,
        rect: edit.rect,
      });
    }
  };
  const addBelow = () => {
    commit((state, now) =>
      now.kind === "code-actions" ? paragraphBelowChanges(state, now.at) : null,
    );
  };
  return (
    <>
      <ActionItem name="Code language" onPick={toLanguage} />
      <ActionItem name="Add a paragraph below" onPick={addBelow} />
    </>
  );
}
