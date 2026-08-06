/**
 * The controls that narrow a search.
 *
 * The fields and the chips are the shared ones every filtering screen uses
 * (`FilterControls`); what search adds is the two axes only it has - which
 * domains to sweep, and how the engine should rank - and the translation from a
 * control to a change the screen can write to the URL.
 *
 * Domains are chips rather than a typed list because the instance enumerates
 * them: the set on screen is every domain there is, and none of them selected
 * means all of them, which is the engine's own default rather than a shortcut.
 */

import type { SearchMode, SearchRequest } from "../api/search";
import { SEARCH_MODES, hasSearchFilters } from "../api/search";
import type { TagCount } from "../api/vocabulary";
import {
  Chip,
  ChipRow,
  FIELD_CLASSES,
  FilterFields,
  TagChips,
} from "./FilterControls";

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
        <FilterFields
          // Keyed by what is applied, so the fields reset to it whenever the
          // URL moves under this screen: a back button, or a shared link.
          key={`${request.type ?? ""}|${request.status ?? ""}|${request.after ?? ""}`}
          type={request.type}
          status={request.status}
          after={request.after}
          clearable={hasSearchFilters(request)}
          onApply={({ type, status, after }) => {
            onChange({ type, status, after });
          }}
          onClear={() => {
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

      <TagChips
        tags={tags}
        chosen={request.tags}
        onChange={(next) => {
          onChange({ tags: next });
        }}
      />
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
