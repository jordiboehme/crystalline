/**
 * The "Task state" menu: the three states of a task box, the current one
 * checked. Picking one writes the one character of the box (`taskChanges`,
 * the writer the click and Mod-Enter share); picking the current one writes
 * nothing.
 */

import type { ReactElement } from "react";
import { useState } from "react";

import { ITEM_CLASSES } from "../../components/menu";
import { taskAt } from "../elementVerbs";
import type { TaskState } from "../marks";
import { taskChanges } from "../marks";
import { editAt } from "../elementEdit";
import type { PopoverBodyProps } from "./ElementPopover";

const STATES: { state: TaskState; name: string }[] = [
  { state: "open", name: "Open" },
  { state: "done", name: "Done" },
  { state: "na", name: "Not applicable" },
];

/** One choice of a menu, checked or not. */
export function RadioItem({
  name,
  checked,
  onPick,
}: {
  name: string;
  checked: boolean;
  onPick: () => void;
}): ReactElement {
  return (
    <button
      type="button"
      role="menuitemradio"
      aria-checked={checked}
      tabIndex={-1}
      className={`${ITEM_CLASSES} w-full text-left`}
      onClick={onPick}
    >
      {name}
    </button>
  );
}

/** The state the box at `at` holds. */
function stateAt(view: PopoverBodyProps["view"], at: number): TaskState {
  const box = taskAt(view.state, at);
  const middle = box
    ? view.state.doc.sliceString(box.from + 1, box.from + 2)
    : " ";
  if (middle === "~") return "na";
  return middle === "x" || middle === "X" ? "done" : "open";
}

export function TaskMenu({
  view,
  edit,
  commit,
}: PopoverBodyProps): ReactElement {
  const [current] = useState(() => stateAt(view, editAt(edit)));
  const pick = (state: TaskState) => {
    if (state === current) {
      commit(() => []);
      return;
    }
    commit((doc, now) => {
      if (now.kind !== "task") return null;
      const change = taskChanges(doc, now.at, state);
      return change ? [change] : null;
    });
  };
  return (
    <>
      {STATES.map(({ state, name }) => (
        <RadioItem
          key={state}
          name={name}
          checked={state === current}
          onPick={() => {
            pick(state);
          }}
        />
      ))}
    </>
  );
}
