/**
 * The people picker: a login field that finds accounts as you type.
 *
 * It asks `GET /accounts/search` for the domain the card belongs to, at
 * least two characters and 300 ms after the last key, and lists up to ten
 * accounts by display name and login. An account that cannot be picked here
 * stays in the list with the reason beside it ("owner", "already a
 * member"), so a person who looked for someone learns why they are not
 * offered. The search answers only `member`; the words are this side's.
 *
 * The field still takes a typed login. Nothing in the list is active until
 * an arrow key makes it so, and Enter on a field with no active row submits
 * the form around it with what was typed, as before. The server decides in
 * the end either way.
 *
 * When a settled search finds nobody, the line under the field says so, and
 * the caller may put an action beside it ("Invite someone new"). That line
 * does not depend on the focus, so a keyboard user can tab to the action.
 *
 * The list is drawn the way `SuggestInput` draws its own (same panel, same
 * keyboard), and Escape on an open list closes the list only, so a confirm
 * around the field stays open.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";

import { ApiProblem, problemDetail } from "../api/client";
import {
  SEARCH_MIN_CHARS,
  accountSearchKey,
  searchAccounts,
} from "../api/accounts";
import { sameAccount } from "../api/members";
import type { AccountHit } from "../api/model";
import { MENU_CLASSES } from "./menu";

/** How long the field waits after the last key before it searches. */
export const SEARCH_DELAY_MS = 300;

/** What the picker picks for. */
export type PickFor = "member" | "owner";

/** What a throttled search says: the route allows 60 a minute per caller. */
export const TOO_MANY_SEARCHES =
  "Too many searches. Wait a minute and type again.";

/** No row is active. */
const NO_ROW = -1;

const OPTION = {
  off: "flex cursor-pointer flex-wrap items-baseline gap-x-2 rounded px-2 py-1.5 text-sm text-slate-700 dark:text-slate-300",
  on: "flex cursor-pointer flex-wrap items-baseline gap-x-2 rounded bg-slate-100 px-2 py-1.5 text-sm text-slate-700 dark:bg-slate-800 dark:text-slate-300",
  unavailable:
    "flex cursor-default flex-wrap items-baseline gap-x-2 rounded px-2 py-1.5 text-sm text-slate-500 dark:text-slate-400",
} as const;

/**
 * Why `hit` cannot be picked for `pickFor`, or null when it can. The owner
 * is never a new member nor a new owner; a member is not added twice, but
 * may become the owner.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function unavailableReason(
  hit: AccountHit,
  owner: string | null,
  pickFor: PickFor,
): string | null {
  if (owner !== null && sameAccount(hit.name, owner)) {
    return "owner";
  }
  if (pickFor === "member" && hit.member) {
    return "already a member";
  }
  return null;
}

/** The value `delay` ms after it last changed. */
function useSettled(value: string, delay: number): string {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const timer = window.setTimeout(() => {
      setSettled(value);
    }, delay);
    return () => {
      window.clearTimeout(timer);
    };
  }, [value, delay]);
  return settled;
}

export interface PeoplePickerProps {
  /** The field's id, which its label points at. */
  id: string;
  /** The domain the search is for. */
  domain: string;
  /** The domain's owner, so the list can say so. */
  owner: string | null;
  pickFor: PickFor;
  value: string;
  onChange: (next: string) => void;
  /** Drawn beside "No account matches", for a next step. */
  noMatchAction?: ReactNode;
  readOnly?: boolean;
  autoFocus?: boolean;
  describedBy?: string | undefined;
  className?: string | undefined;
}

export function PeoplePicker({
  id,
  domain,
  owner,
  pickFor,
  value,
  onChange,
  noMatchAction,
  readOnly = false,
  autoFocus = false,
  describedBy,
  className,
}: PeoplePickerProps) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(NO_ROW);
  const typed = value.trim();
  const settled = useSettled(typed, SEARCH_DELAY_MS).trim();
  const searching = !readOnly && settled.length >= SEARCH_MIN_CHARS;
  const search = useQuery({
    queryKey: accountSearchKey(domain, settled),
    queryFn: () => searchAccounts(settled, domain),
    enabled: searching,
    retry: false,
    staleTime: 30_000,
  });
  const hits =
    searching && settled === typed ? (search.data?.results ?? []) : [];
  const expanded = open && hits.length > 0;
  const index = active === NO_ROW ? NO_ROW : Math.min(active, hits.length - 1);
  const hasActive = expanded && index >= 0;
  const settledHere = searching && settled === typed;
  const noMatch =
    settledHere && search.isSuccess && search.data.results.length === 0;
  // A refused search says why rather than showing nothing: a 429 from the
  // rate limit in plain words, anything else in the server's own.
  const failed = settledHere && search.isError ? search.error : null;
  const failure =
    failed === null
      ? null
      : failed instanceof ApiProblem && failed.status === 429
        ? TOO_MANY_SEARCHES
        : problemDetail(failed);
  const listId = `${id}-people`;
  const optionId = (position: number) => `${id}-person-${String(position)}`;

  const pick = (hit: AccountHit) => {
    if (unavailableReason(hit, owner, pickFor) !== null) {
      return;
    }
    setOpen(false);
    setActive(NO_ROW);
    onChange(hit.name);
  };

  return (
    <div className="relative flex flex-col gap-1">
      <input
        id={id}
        role="combobox"
        aria-expanded={expanded}
        {...(expanded ? { "aria-controls": listId } : {})}
        aria-autocomplete="list"
        {...(hasActive ? { "aria-activedescendant": optionId(index) } : {})}
        {...(describedBy !== undefined
          ? { "aria-describedby": describedBy }
          : {})}
        autoFocus={autoFocus}
        autoComplete="off"
        required
        placeholder="name or login"
        readOnly={readOnly}
        value={value}
        className={className ?? ""}
        onChange={(event) => {
          setActive(NO_ROW);
          setOpen(true);
          onChange(event.target.value);
        }}
        onFocus={() => {
          setOpen(true);
        }}
        onBlur={() => {
          setOpen(false);
          setActive(NO_ROW);
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            if (!open) {
              setOpen(true);
              return;
            }
            if (hits.length === 0) return;
            if (event.key === "ArrowDown") {
              setActive(Math.max(0, Math.min(index + 1, hits.length - 1)));
            } else {
              setActive(
                index === NO_ROW ? hits.length - 1 : Math.max(index - 1, 0),
              );
            }
            return;
          }
          if (event.key === "Enter") {
            const chosen = hasActive ? hits[index] : undefined;
            if (chosen) {
              // A row was walked to: Enter picks it (or, for a row that
              // cannot be picked, does nothing) and never submits.
              event.preventDefault();
              pick(chosen);
              return;
            }
            setOpen(false);
            return;
          }
          if (event.key === "Escape" && expanded) {
            // The list only: a confirm around this field stays open.
            event.preventDefault();
            event.stopPropagation();
            setOpen(false);
            setActive(NO_ROW);
          }
        }}
      />
      {expanded && (
        <ul
          id={listId}
          role="listbox"
          aria-label="People found"
          className={`absolute top-full left-0 z-10 mt-1 max-h-64 w-72 max-w-[calc(100vw-2rem)] overflow-y-auto ${MENU_CLASSES}`}
        >
          {hits.map((hit, position) => {
            const reason = unavailableReason(hit, owner, pickFor);
            const on = hasActive && position === index;
            return (
              <li
                key={hit.name}
                id={optionId(position)}
                role="option"
                aria-selected={on}
                aria-disabled={reason !== null}
                data-value={hit.name}
                className={
                  reason !== null
                    ? OPTION.unavailable
                    : on
                      ? OPTION.on
                      : OPTION.off
                }
                onMouseDown={(event) => {
                  // The field keeps the focus through the pick.
                  event.preventDefault();
                }}
                onMouseEnter={() => {
                  setActive(position);
                }}
                onClick={() => {
                  pick(hit);
                }}
              >
                <span className="font-medium" title={hit.name}>
                  {hit.display}
                </span>
                {hit.display !== hit.name && (
                  <span className="text-caption font-mono">{hit.name}</span>
                )}
                {reason !== null && (
                  <span className="text-caption">{reason}</span>
                )}
              </li>
            );
          })}
        </ul>
      )}
      <div className="flex flex-wrap items-center gap-2 text-sm">
        {/*
          Only the sentence is live, so the action beside it is not read out
          with it; the region stays mounted so a new sentence is announced.
        */}
        <span aria-live="polite" className="text-slate-500 dark:text-slate-400">
          {failure ?? (noMatch ? "No account matches." : "")}
        </span>
        {(noMatch || failure !== null) && noMatchAction}
      </div>
    </div>
  );
}
