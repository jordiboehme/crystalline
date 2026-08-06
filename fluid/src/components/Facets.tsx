/**
 * The controls that narrow a search.
 *
 * Two kinds of control, and the difference is deliberate. What is picked -
 * the mode, a domain, a tag - applies the moment it is clicked, because the
 * click already said everything the screen needs. What is typed - a type, a
 * status, a date - applies when the form is submitted, because a filter that
 * refired on every keystroke would search for `dec`, `deci` and `decis` on the
 * way to `decision`.
 *
 * Which axes are real sets and which are suggestions is the server's answer
 * rather than a design choice. Domains and tags are enumerable, so they are
 * chips and the set on screen is the whole truth. `type` and `status` are free
 * form and nothing lists the values in use, so they are typed with a datalist
 * beside them: anything is allowed, and nothing here says otherwise.
 */

import { useState } from "react";
import type { ReactNode } from "react";

import type { SearchMode, SearchRequest } from "../api/search";
import { SEARCH_MODES, hasSearchFilters } from "../api/search";
import type { TagCount } from "../api/vocabulary";
import { SUGGESTED_STATUSES, SUGGESTED_TYPES } from "../filters";

/** The classes every text-ish input in here shares. */
const FIELD_CLASSES =
  "rounded border border-slate-300 bg-white px-2 py-1 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100";

/** The classes a chip shares, on or off. */
const CHIP_CLASSES =
  "rounded-full border px-2 py-0.5 text-xs focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none";
const CHIP_ON =
  "border-sky-600 bg-sky-50 text-sky-800 dark:bg-sky-950 dark:text-sky-200";
const CHIP_OFF =
  "border-slate-200 hover:bg-slate-100 dark:border-slate-800 dark:hover:bg-slate-800";

/** What a facet change says: only the axes it names, each one whole. */
export interface FacetChange {
  domains?: string[];
  type?: string | null;
  status?: string | null;
  tags?: string[];
  after?: string | null;
  mode?: SearchMode;
}

export interface FacetsProps {
  /** The search as it stands, which is the URL. */
  request: SearchRequest;
  /** Every registered domain, for the chips. */
  domains: string[];
  /** The tags in use, commonest first. */
  tags: TagCount[];
  /** Apply a change. The screen writes it to the URL; this never navigates. */
  onChange: (next: FacetChange) => void;
}

export function Facets({ request, domains, tags, onChange }: FacetsProps) {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end gap-3">
        <ModeSelect
          mode={request.mode}
          onChange={(mode) => {
            onChange({ mode });
          }}
        />
        <TypedFilters
          // Keyed by what is applied, so the fields reset to it whenever the
          // URL moves under this screen: a back button, or a shared link.
          key={`${request.type ?? ""}|${request.status ?? ""}|${request.after ?? ""}`}
          request={request}
          onChange={onChange}
        />
      </div>

      {domains.length > 0 && (
        <ChipRow label="Domains">
          {domains.map((name) => {
            const on = request.domains.includes(name);
            return (
              <Chip
                key={name}
                on={on}
                onClick={() => {
                  onChange({
                    domains: on
                      ? request.domains.filter((each) => each !== name)
                      : [...request.domains, name],
                  });
                }}
              >
                {name}
              </Chip>
            );
          })}
        </ChipRow>
      )}

      {tags.length > 0 && (
        <ChipRow label="Tags">
          {tags.map((tag) => {
            const on = request.tags.includes(tag.name);
            return (
              <Chip
                key={tag.name}
                on={on}
                onClick={() => {
                  onChange({
                    tags: on
                      ? request.tags.filter((each) => each !== tag.name)
                      : [...request.tags, tag.name],
                  });
                }}
              >
                <span>#{tag.name}</span>{" "}
                <span className="text-slate-500 tabular-nums dark:text-slate-400">
                  {tag.engrams}
                </span>
              </Chip>
            );
          })}
        </ChipRow>
      )}
    </div>
  );
}

/**
 * How the engine should rank.
 *
 * Hybrid is the default because it is the engine's, and picking another is a
 * claim about this query rather than a preference: title when the words are a
 * name, text when they are exact, semantic when they are a description of
 * something whose words nobody remembers.
 */
function ModeSelect({
  mode,
  onChange,
}: {
  mode: SearchMode;
  onChange: (mode: SearchMode) => void;
}) {
  return (
    <label className="flex flex-col gap-1 text-xs text-slate-500 dark:text-slate-400">
      Mode
      <select
        value={mode}
        onChange={(event) => {
          onChange(event.target.value as SearchMode);
        }}
        className={FIELD_CLASSES}
      >
        {SEARCH_MODES.map((value) => (
          <option key={value} value={value}>
            {value}
          </option>
        ))}
      </select>
    </label>
  );
}

/** The filters that are written rather than picked, applied together. */
function TypedFilters({
  request,
  onChange,
}: {
  request: SearchRequest;
  onChange: (next: FacetChange) => void;
}) {
  const [type, setType] = useState(request.type ?? "");
  const [status, setStatus] = useState(request.status ?? "");
  const [after, setAfter] = useState(request.after ?? "");

  return (
    <form
      className="flex flex-wrap items-end gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        onChange({
          type: type.trim(),
          status: status.trim(),
          after: after.trim(),
        });
      }}
    >
      <label className="flex flex-col gap-1 text-xs text-slate-500 dark:text-slate-400">
        Type
        <input
          list="search-types"
          value={type}
          onChange={(event) => {
            setType(event.target.value);
          }}
          className={`w-36 ${FIELD_CLASSES}`}
        />
        <datalist id="search-types">
          {SUGGESTED_TYPES.map((value) => (
            <option key={value} value={value} />
          ))}
        </datalist>
      </label>

      <label className="flex flex-col gap-1 text-xs text-slate-500 dark:text-slate-400">
        Status
        <input
          list="search-statuses"
          value={status}
          onChange={(event) => {
            setStatus(event.target.value);
          }}
          className={`w-36 ${FIELD_CLASSES}`}
        />
        <datalist id="search-statuses">
          {SUGGESTED_STATUSES.map((value) => (
            <option key={value} value={value} />
          ))}
        </datalist>
      </label>

      <label className="flex flex-col gap-1 text-xs text-slate-500 dark:text-slate-400">
        Recorded after
        <input
          type="date"
          value={after}
          onChange={(event) => {
            setAfter(event.target.value);
          }}
          className={`w-40 ${FIELD_CLASSES}`}
        />
      </label>

      <button
        type="submit"
        className="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
      >
        Apply
      </button>

      {hasSearchFilters(request) && (
        <button
          type="button"
          className="rounded px-2 py-1 text-sm underline underline-offset-2 hover:no-underline"
          onClick={() => {
            setType("");
            setStatus("");
            setAfter("");
            // The query itself survives: clearing the filters widens a search
            // rather than ending it.
            onChange({
              domains: [],
              type: null,
              status: null,
              tags: [],
              after: null,
            });
          }}
        >
          Clear filters
        </button>
      )}
    </form>
  );
}

function ChipRow({ label, children }: { label: string; children: ReactNode }) {
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

function Chip({
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
