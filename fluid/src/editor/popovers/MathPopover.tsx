/**
 * The "Formula" popover: the LaTeX of a formula, in a one-line field for
 * inline math and a textarea for a block, with a live Preview set by the
 * reading page's lazy `MathPart` (the text shows until KaTeX arrives, and a
 * formula KaTeX cannot read shows its message). Save or Enter (Mod-Enter in
 * the textarea) writes only the formula between its delimiters; "Remove
 * formula" takes the whole element away. A formula that would close its own
 * delimiter cannot be saved, and a line under the field says what to remove.
 */

import type { ReactElement } from "react";
import { useId, useState } from "react";

import { lazyPart } from "../../components/lazyPart";
import { BUTTON, FIELD } from "../../components/primitives";
import type { FormulaRefusal } from "../elementVerbs";
import {
  formulaRefusal,
  mathAt,
  mathChanges,
  removeMathChanges,
} from "../elementVerbs";
import { mathSections } from "../mathBlocks";
import { editAt } from "../elementEdit";
import type { PopoverBodyProps } from "./ElementPopover";

/** The reading page's formula part: its own chunk, KaTeX inside it. */
const MathPart = lazyPart(() => import("../../components/MathPart"));

/** What to remove, for each formula that cannot be saved as typed. */
const WHY: Record<Exclude<FormulaRefusal, "empty">, string> = {
  "line-break": "Remove its line breaks.",
  backtick: "Remove its backticks.",
  dollars: "Remove its $$ signs.",
  "dollar-edge": "Remove the $ at its start or end.",
  "dollar-line": "Remove the line that holds only $$.",
  "fence-line": "Remove the line of backticks.",
  "heading-line": "Remove the ## at the start of its lines.",
};

export function MathPopover({
  view,
  edit,
  commit,
}: PopoverBodyProps): ReactElement | null {
  const id = useId();
  const [initial] = useState(() => mathAt(view.state, editAt(edit)));
  const [formula, setFormula] = useState(initial?.formula ?? "");
  if (edit.kind !== "math" || initial === null) return null;
  const unchanged = formula === initial.formula;
  const refusal = unchanged
    ? null
    : formulaRefusal(
        initial.form,
        formula,
        view.state.facet(mathSections),
        initial.prefix !== "",
      );
  const why = refusal !== null && refusal !== "empty" ? WHY[refusal] : null;

  const save = () => {
    if (refusal !== null) return;
    if (unchanged) {
      commit(() => []);
      return;
    }
    commit((state, now) =>
      now.kind === "math" ? mathChanges(state, now.at, { formula }) : null,
    );
  };
  const remove = () => {
    commit((state, now) =>
      now.kind === "math" ? removeMathChanges(state, now.at) : null,
    );
  };
  const field = {
    id: `${id}-tex`,
    value: formula,
    spellCheck: false,
    autoComplete: "off",
    ...(why !== null ? { "aria-describedby": `${id}-why` } : {}),
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
        <label htmlFor={`${id}-tex`}>LaTeX</label>
        {initial.block ? (
          <textarea
            {...field}
            rows={4}
            className={`${FIELD} h-auto py-1 font-mono`}
            onChange={(event) => {
              setFormula(event.target.value);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                event.preventDefault();
                save();
              }
            }}
          />
        ) : (
          <input
            {...field}
            className={`${FIELD} font-mono`}
            onChange={(event) => {
              setFormula(event.target.value);
            }}
          />
        )}
        {why !== null && (
          <p
            id={`${id}-why`}
            className="text-caption text-slate-600 dark:text-slate-300"
          >
            {`This formula cannot be saved. ${why}`}
          </p>
        )}
      </div>
      <div
        role="region"
        aria-label="Preview"
        className="min-h-8 overflow-x-auto rounded border border-slate-200 px-2 py-1 dark:border-slate-700"
      >
        <MathPart
          tex={formula}
          display={initial.block}
          report
          fallback={<code>{formula}</code>}
        />
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="submit"
          className={BUTTON.primary}
          disabled={refusal !== null}
        >
          Save
        </button>
        <button type="button" className={BUTTON.ghost} onClick={remove}>
          Remove formula
        </button>
      </div>
    </form>
  );
}
