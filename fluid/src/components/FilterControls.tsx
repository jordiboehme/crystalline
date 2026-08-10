/**
 * The controls a filtering screen is built from.
 *
 * Two screens filter engrams - a domain's listing and search - and they filter
 * on the same axes with the same rules, so the controls live here once. What
 * each screen keeps is what a change means to it: the domain screen writes a
 * folder-or-filter URL, search writes a query URL, and neither of those
 * decisions belongs to a text field.
 *
 * The split between the two kinds of control is the point of the design. What
 * is picked - a tag chip - applies on the click, because the click already said
 * everything. What is typed applies on submit, because a filter that refired on
 * every keystroke would search for `dec`, `deci` and `decis` on the way to
 * `decision`. The fields hold their own draft until then, which is the one piece
 * of state in here and the reason it is a component rather than a snippet of
 * markup.
 *
 * Which axes are a real set and which are suggestions is the server's answer
 * rather than a design choice: tags are enumerable through `/vocabulary`, so
 * they are chips and the set on screen is the whole truth, while `type` and
 * `status` are free form with nothing listing the values in use, so they are
 * typed with a datalist beside them.
 */

import { useState } from "react";
import type { ReactNode } from "react";

import type { TagCount } from "../api/vocabulary";
import { SUGGESTED_STATUSES, SUGGESTED_TYPES } from "../filters";

/** The classes every text-ish input and select in a filter bar shares. */
export const FIELD_CLASSES =
  "rounded border border-slate-300 bg-white px-2 py-1 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100";

/** The classes every chip shares, on or off. */
const CHIP_CLASSES =
  "flex items-baseline gap-1 rounded-full border px-2 py-0.5 text-xs focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none";
const CHIP_ON =
  "border-sky-600 bg-sky-50 text-sky-800 dark:bg-sky-950 dark:text-sky-200";
const CHIP_OFF =
  "border-slate-200 hover:bg-slate-100 dark:border-slate-800 dark:hover:bg-slate-800";

/** What the fields say when they are applied. Unset axes are empty strings. */
export interface AppliedFields {
  type: string;
  status: string;
  after: string;
}

export interface FilterFieldsProps {
  /** The applied `type`, which the field opens on. */
  type: string | null;
  /** The applied `status`, which the field opens on. */
  status: string | null;
  /**
   * The applied recorded-after day. Omit the prop entirely on a screen with no
   * timeframe axis; `null` means the axis is there and nothing is set.
   */
  after?: string | null;
  /** Whether anything is filtered, which is what the clear button is for. */
  clearable: boolean;
  /** Apply what is in the fields. */
  onApply: (applied: AppliedFields) => void;
  /** Drop every filter. The fields empty themselves before this is called. */
  onClear: () => void;
}

/** The filters that are written rather than picked, applied together. */
export function FilterFields({
  type: appliedType,
  status: appliedStatus,
  after: appliedAfter,
  clearable,
  onApply,
  onClear,
}: FilterFieldsProps) {
  const [type, setType] = useState(appliedType ?? "");
  const [status, setStatus] = useState(appliedStatus ?? "");
  const [after, setAfter] = useState(appliedAfter ?? "");
  const timeframe = appliedAfter !== undefined;

  return (
    <form
      className="flex flex-wrap items-end gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        onApply({
          type: type.trim(),
          status: status.trim(),
          after: after.trim(),
        });
      }}
    >
      <Field label="Type">
        <input
          list="filter-types"
          value={type}
          onChange={(event) => {
            setType(event.target.value);
          }}
          className={`w-40 ${FIELD_CLASSES}`}
        />
        <datalist id="filter-types">
          {SUGGESTED_TYPES.map((value) => (
            <option key={value} value={value} />
          ))}
        </datalist>
      </Field>

      <Field label="Status">
        <input
          list="filter-statuses"
          value={status}
          onChange={(event) => {
            setStatus(event.target.value);
          }}
          className={`w-40 ${FIELD_CLASSES}`}
        />
        <datalist id="filter-statuses">
          {SUGGESTED_STATUSES.map((value) => (
            <option key={value} value={value} />
          ))}
        </datalist>
      </Field>

      {timeframe && (
        <Field label="Recorded after">
          <input
            type="date"
            value={after}
            onChange={(event) => {
              setAfter(event.target.value);
            }}
            className={`w-40 ${FIELD_CLASSES}`}
          />
        </Field>
      )}

      <button
        type="submit"
        className="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
      >
        Apply
      </button>

      {clearable && (
        <button
          type="button"
          className="rounded px-2 py-1 text-sm underline underline-offset-2 hover:no-underline"
          onClick={() => {
            setType("");
            setStatus("");
            setAfter("");
            onClear();
          }}
        >
          Clear filters
        </button>
      )}
    </form>
  );
}

/** One labelled control in a filter bar. */
function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1 text-xs text-slate-500 dark:text-slate-400">
      {label}
      {children}
    </label>
  );
}

/** A labelled row of chips. */
export function ChipRow({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-baseline gap-2">
      <span className="text-xs text-slate-500 dark:text-slate-400">
        {label}
      </span>
      <ul aria-label={label} className="flex flex-wrap gap-1.5">
        {children}
      </ul>
    </div>
  );
}

/** One chip: a filter that is on or off, and says which. */
export function Chip({
  on,
  onClick,
  children,
}: {
  on: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <li>
      <button
        type="button"
        aria-pressed={on}
        onClick={onClick}
        className={`${CHIP_CLASSES} ${on ? CHIP_ON : CHIP_OFF}`}
      >
        {children}
      </button>
    </li>
  );
}

/**
 * The tags in use, with how many engrams carry each.
 *
 * The count is on the chip because it is what makes the row usable: it says
 * which tags are the vocabulary of this knowledge base and which were used once
 * and forgotten.
 */
export function TagChips({
  tags,
  chosen,
  onChange,
}: {
  tags: TagCount[];
  chosen: string[];
  onChange: (tags: string[]) => void;
}) {
  if (tags.length === 0) {
    return null;
  }
  return (
    <ChipRow label="Tags">
      {tags.map((tag) => {
        const on = chosen.includes(tag.name);
        return (
          <Chip
            key={tag.name}
            on={on}
            onClick={() => {
              onChange(
                on
                  ? chosen.filter((name) => name !== tag.name)
                  : [...chosen, tag.name],
              );
            }}
          >
            <span>#{tag.name}</span>
            <span className="text-slate-500 tabular-nums dark:text-slate-400">
              {tag.engrams}
            </span>
          </Chip>
        );
      })}
    </ChipRow>
  );
}
