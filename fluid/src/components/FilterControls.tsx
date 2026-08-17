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
 * they are chips and the set in hand is the whole truth, while `type` and
 * `status` are free form with nothing listing the values in use, so they are
 * typed with a datalist beside them. In hand rather than on screen, because a
 * well-taught domain is written in more tags than any rail can draw: the whole
 * vocabulary is here, and {@link TagChips} caps what it draws and reaches the
 * rest by narrowing.
 */

import { useRef, useState } from "react";
import type { ReactNode } from "react";

import type { TagCount } from "../api/vocabulary";
import { SUGGESTED_STATUSES, SUGGESTED_TYPES } from "../filters";
import { BUTTON, FOCUS_RING } from "./primitives";

/** The classes every text-ish input and select in a filter bar shares. */
export const FIELD_CLASSES =
  "h-8 rounded border border-slate-300 bg-white px-2 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100";

/** The classes every chip shares, on or off. */
const CHIP_CLASSES =
  "flex items-baseline gap-1 rounded-full border px-2 py-0.5 text-xs focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none";
const CHIP_ON =
  "border-accent-600 bg-accent-50 text-accent-800 dark:bg-accent-950 dark:text-accent-200";
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
      // One row, one control height: the fields, Apply and Clear all
      // sit on the same line rather than at three heights the browser
      // happened to give them. Labels stack over their own field, so the row
      // aligns on the bottom edge - which is where the controls themselves
      // are - rather than through the middle of a label.
      className="flex flex-wrap items-end gap-2"
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

      <button type="submit" className={`h-8 ${BUTTON.secondary}`}>
        Apply
      </button>

      {clearable && (
        <button
          type="button"
          className={`h-8 rounded px-2 text-sm underline underline-offset-2 hover:no-underline ${FOCUS_RING}`}
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
  control,
  listClassName = "",
  children,
}: {
  label: string;
  /** A control that belongs to the row itself, drawn beside its label. */
  control?: ReactNode;
  /** What to add to the chip list, for a row that has to bound its height. */
  listClassName?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-baseline gap-2">
      <span className="text-xs text-slate-500 dark:text-slate-400">
        {label}
      </span>
      {control}
      {/* pb-2 so a wrapped row's chips keep their focus ring: the row sits
          in a column whose next line would otherwise clip it. */}
      <ul
        aria-label={label}
        className={`flex flex-wrap gap-1 pb-2 ${listClassName}`}
      >
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
 * How many chips the rail draws before it starts hiding the rest.
 *
 * A cap rather than the whole vocabulary, because a domain that has been taught
 * for a year is written in a few hundred tags and a chip for each of them is a
 * wall that pushes the results below the fold. Twelve is what fits on two lines
 * at a usual width, which is the most a facet rail can cost before it stops
 * being a rail.
 */
export const MAX_VISIBLE_TAGS = 12;

/**
 * The tags in use, with how many engrams carry each.
 *
 * The count is on the chip because it is what makes the row usable: it says
 * which tags are the vocabulary of this knowledge base and which were used once
 * and forgotten.
 *
 * What is past the cap is reached by narrowing rather than by expanding. An
 * expander would put the wall back one click away; the filter answers the
 * question a reader with three hundred tags actually has, which is "is there a
 * tag for this", and it costs no request because the whole vocabulary is
 * already in hand. Two rules keep the cap honest: a chosen tag is drawn first
 * and so is never the one hidden, and while the filter is narrowing, the
 * matches live in a box that scrolls rather than in a page that grows.
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
  const [filter, setFilter] = useState("");
  const box = useRef<HTMLInputElement>(null);

  const needle = filter.trim().toLowerCase();
  const narrowing = needle !== "";
  // Selected first, in the order they were turned on, then the vocabulary as
  // it arrived - which is commonest first, so this is a slice and not a sort.
  const byName = new Map(tags.map((tag) => [tag.name, tag]));
  const ordered = [
    ...chosen
      .map((name) => byName.get(name))
      .filter((tag) => tag !== undefined),
    ...tags.filter((tag) => !chosen.includes(tag.name)),
  ];
  const shown = narrowing
    ? ordered.filter((tag) => tag.name.toLowerCase().includes(needle))
    : ordered.slice(0, MAX_VISIBLE_TAGS);
  const overflowing = tags.length > MAX_VISIBLE_TAGS;
  const hidden = tags.length - shown.length;

  if (tags.length === 0) {
    return null;
  }

  return (
    <ChipRow
      label="Tags"
      listClassName={narrowing ? "max-h-32 overflow-y-auto" : ""}
      {...(overflowing
        ? {
            control: (
              <span className="flex items-baseline gap-2">
                <input
                  ref={box}
                  type="search"
                  value={filter}
                  aria-label="Filter tags"
                  placeholder="Filter tags"
                  onChange={(event) => {
                    setFilter(event.target.value);
                  }}
                  className="h-6 w-36 rounded border border-slate-300 bg-white px-2 text-xs text-slate-900 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
                />
                {!narrowing && hidden > 0 && (
                  <button
                    type="button"
                    // The count is the affordance: it says what is missing and
                    // hands the reader the one control that reaches it.
                    onClick={() => {
                      box.current?.focus();
                    }}
                    className={`rounded text-xs text-slate-500 underline underline-offset-2 hover:no-underline dark:text-slate-400 ${FOCUS_RING}`}
                  >
                    +{hidden} more
                  </button>
                )}
              </span>
            ),
          }
        : {})}
    >
      {shown.map((tag) => {
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
      {shown.length === 0 && (
        <li className="text-xs text-slate-500 dark:text-slate-400">
          no tag matches
        </li>
      )}
    </ChipRow>
  );
}
