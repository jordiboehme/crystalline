/**
 * The "Status label" popover: a label's words and its colour. Save or Enter
 * writes only what the person changed (the words span, the colour word, or
 * both in one change list), so a peer's change to the other one stays. Empty
 * words cannot be saved.
 */

import type { ReactElement } from "react";
import { useId, useState } from "react";

import { BUTTON, FIELD } from "../../components/primitives";
import type { StatusColour } from "../../markdown/directives";
import { STATUS_COLOURS } from "../../markdown/directives";
import {
  statusAt,
  statusChanges,
  statusWords,
  writtenLabel,
} from "../elementVerbs";
import { editAt } from "../elementEdit";
import type { PopoverBodyProps } from "./ElementPopover";

export function StatusPopover({
  view,
  edit,
  commit,
}: PopoverBodyProps): ReactElement {
  const id = useId();
  const [initial] = useState(() => ({
    words: statusWords(view.state, editAt(edit)) ?? "",
    colour: statusAt(view.state, editAt(edit))?.colour ?? "grey",
  }));
  const [words, setWords] = useState(initial.words);
  const [colour, setColour] = useState<StatusColour>(initial.colour);
  const wordsChanged = words !== initial.words;
  const valid = !wordsChanged || writtenLabel(words) !== null;

  const save = () => {
    if (!valid) return;
    // Only what the person changed is written.
    const changed = {
      ...(wordsChanged ? { words } : {}),
      ...(colour !== initial.colour ? { colour } : {}),
    };
    if (Object.keys(changed).length === 0) {
      commit(() => []);
      return;
    }
    commit((state, now) =>
      now.kind === "status" ? statusChanges(state, now.at, changed) : null,
    );
  };

  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        save();
      }}
    >
      <div className="flex flex-col gap-1">
        <label htmlFor={`${id}-label`}>Label</label>
        <input
          id={`${id}-label`}
          className={FIELD}
          value={words}
          autoComplete="off"
          onChange={(event) => {
            setWords(event.target.value);
          }}
        />
      </div>
      <div className="flex flex-col gap-1">
        <span id={`${id}-colour`}>Colour</span>
        <div
          role="radiogroup"
          aria-labelledby={`${id}-colour`}
          className="flex flex-wrap gap-1.5"
        >
          {STATUS_COLOURS.map((word) => (
            <label key={word} className="inline-flex items-center gap-1">
              <input
                type="radio"
                name={`${id}-colours`}
                value={word}
                checked={colour === word}
                onChange={() => {
                  setColour(word);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    save();
                  }
                }}
              />
              <span className="status-label" data-status={word}>
                {word}
              </span>
            </label>
          ))}
        </div>
      </div>
      <div className="flex items-center gap-2">
        <button type="submit" className={BUTTON.primary} disabled={!valid}>
          Save
        </button>
      </div>
    </form>
  );
}
