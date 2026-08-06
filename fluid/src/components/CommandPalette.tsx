/**
 * The keyboard's way around the app.
 *
 * Cmd+K, or Ctrl+K on a keyboard without a Cmd key, anywhere in the app. What
 * it offers is the two things this instance is addressed by: the domains it
 * holds, filtered here because the whole list already arrived with the sidebar,
 * and engrams by title, which the server answers because only the index knows
 * what is in the other domains.
 *
 * Titles only, on purpose. A palette is for reaching something whose name you
 * half remember, and a body-text match dressed as a title would send a reader
 * somewhere they did not ask for. What that leaves out is the last row: a query
 * with no title behind it is handed to the search screen, which searches the
 * text this never asked about. So the palette is never a dead end, and it never
 * pretends to have looked further than it did.
 *
 * Typing is debounced at the same pause the search screen uses, so a query is
 * one request rather than one per keystroke.
 */

import { useQuery } from "@tanstack/react-query";
import { Command } from "cmdk";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";

import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { EngramRow } from "../api/engrams";
import type { SearchRequest } from "../api/search";
import {
  NO_SEARCH,
  SEARCH_DEBOUNCE_MS,
  fetchSearch,
  titleMatchesKey,
} from "../api/search";
import { domainRoute, engramRoute, searchRoute } from "../paths";

/**
 * How many title matches the palette shows.
 *
 * A palette is a list somebody reads without scrolling: past a screenful the
 * search screen is the better tool, and the last row leads there.
 */
const PALETTE_HITS = 7;

/** The classes one row shares, highlighted or not. */
const ROW_CLASSES =
  "flex cursor-pointer items-baseline gap-2 rounded px-2 py-1.5 text-sm outline-none select-none data-[selected=true]:bg-slate-100 dark:data-[selected=true]:bg-slate-800";

/**
 * The classes one group heading shares.
 *
 * On a span inside the heading rather than on the group, because a group wraps
 * its rows: heading type set on the group would inherit straight into them.
 */
const HEADING_CLASSES =
  "block px-2 pt-2 pb-1 text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400";

/** One group heading, drawn the same way wherever it appears. */
function Heading({ children }: { children: string }) {
  return <span className={HEADING_CLASSES}>{children}</span>;
}

/** What one row is called, which is how the highlight keeps track of it. */
function domainValue(name: string): string {
  return `domain:${name}`;
}

function engramValue(hit: EngramRow): string {
  return `engram:${hit.domain}/${hit.permalink}`;
}

function searchValue(term: string): string {
  return `search:${term}`;
}

export function CommandPalette() {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  // What the last pause settled on, which is what the server was asked for.
  const [term, setTerm] = useState("");
  // The row somebody moved the highlight onto, remembered together with the
  // top row it was chosen against. Both halves matter: rows arrive after the
  // typing that asked for them, so a highlight left where it was would still
  // be sitting on the one row there was while the answer landed above it, and
  // a highlight snapped to the top on every render would undo the arrow keys.
  const [choice, setChoice] = useState({ top: "", value: "" });
  // Whatever had the focus when the shortcut fired, so closing can hand it
  // back. A dialog opened by a key has no trigger to return to on its own.
  const invoker = useRef<HTMLElement | null>(null);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      // Either modifier: one keyboard has a Cmd key and the other does not,
      // and a reader should not have to know which one this app was built on.
      if (
        event.key.toLowerCase() !== "k" ||
        !(event.metaKey || event.ctrlKey)
      ) {
        return;
      }
      event.preventDefault();
      setOpen((was) => {
        if (!was) {
          invoker.current =
            document.activeElement instanceof HTMLElement
              ? document.activeElement
              : null;
        }
        return !was;
      });
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  useEffect(() => {
    if (open) {
      return;
    }
    const back = invoker.current;
    invoker.current = null;
    // Only where it still exists: a jump replaces the screen the palette was
    // opened from, and focusing what is left of it would be focusing nothing.
    if (back?.isConnected === true) {
      back.focus();
    }
  }, [open]);

  useEffect(() => {
    const pending = query.trim();
    if (pending === term) {
      return;
    }
    const timer = setTimeout(() => {
      setTerm(pending);
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [query, term]);

  // The listing the sidebar already read, under the same key: opening the
  // palette costs nothing on the wire.
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });

  const request: SearchRequest = { ...NO_SEARCH, q: term, mode: "title" };
  const titles = useQuery({
    queryKey: titleMatchesKey(term),
    queryFn: () => fetchSearch(request, 1),
    // Nothing typed is nothing to look up, and a shut palette looks nothing up
    // at all.
    enabled: open && term !== "",
  });

  // What is in the box, and the same thing folded for the one match this app
  // makes itself. The server is what matches titles, and it decides how; the
  // domain names are matched here, where a reader holding shift is asking the
  // same question as one who is not.
  const typed = query.trim();
  const domains = (listing.data?.domains ?? []).filter((domain) =>
    domain.name.toLowerCase().includes(typed.toLowerCase()),
  );
  // Only the matches for the query on screen: the request behind them is a
  // pause old, and rows from the query before it would point at answers to a
  // question nobody is asking any more.
  const hits =
    term === typed ? (titles.data?.hits ?? []).slice(0, PALETTE_HITS) : [];

  // The top row, in the order they are drawn in. Enter follows the highlight,
  // and the highlight is the top row until somebody moves it off: an answer
  // landing above a highlighted row takes the highlight with it.
  const top =
    domains.length > 0
      ? domainValue(domains[0].name)
      : hits.length > 0
        ? engramValue(hits[0])
        : typed === ""
          ? ""
          : searchValue(typed);
  const highlighted = choice.top === top ? choice.value : top;

  /** Go, and get out of the way. */
  function go(to: string): void {
    setOpen(false);
    void navigate(to);
  }

  return (
    <Command.Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) {
          // A palette that reopens onto the last query is a palette that
          // answers the question before it, so it forgets on the way out.
          setQuery("");
          setTerm("");
        }
      }}
      label="Command palette"
      // The rows are what the server found, so cmdk's own scoring is off: a
      // second filter over an answer would hide rows the index chose to send.
      shouldFilter={false}
      value={highlighted}
      onValueChange={(value) => {
        setChoice({ top, value });
      }}
      overlayClassName="fixed inset-0 z-50 bg-slate-900/40"
      contentClassName="fixed top-24 left-1/2 z-50 w-[min(36rem,calc(100vw-2rem))] -translate-x-1/2 overflow-hidden rounded border border-slate-200 bg-white shadow-xl dark:border-slate-700 dark:bg-slate-900"
    >
      <Command.Input
        value={query}
        onValueChange={setQuery}
        placeholder="Jump to a domain, or find an engram by title"
        className="w-full border-b border-slate-200 bg-transparent px-3 py-3 text-sm outline-none dark:border-slate-800"
      />
      <Command.List
        label="Jump to"
        className="max-h-80 overflow-y-auto p-1.5 text-slate-900 dark:text-slate-100"
      >
        {domains.length > 0 && (
          <Command.Group heading={<Heading>Domains</Heading>}>
            {domains.map((domain) => (
              <Command.Item
                key={domain.name}
                value={domainValue(domain.name)}
                onSelect={() => {
                  go(domainRoute(domain.name));
                }}
                className={ROW_CLASSES}
              >
                <span className="truncate">{domain.name}</span>
                {domain.engrams !== null && (
                  <span className="text-xs text-slate-500 tabular-nums dark:text-slate-400">
                    {domain.engrams}
                  </span>
                )}
              </Command.Item>
            ))}
          </Command.Group>
        )}

        {hits.length > 0 && (
          <Command.Group heading={<Heading>Engrams</Heading>}>
            {hits.map((hit) => (
              <Command.Item
                key={`${hit.domain}/${hit.permalink}`}
                value={engramValue(hit)}
                onSelect={() => {
                  go(engramRoute(hit.domain, hit.permalink));
                }}
                className={ROW_CLASSES}
              >
                <span className="truncate">{hit.title}</span>
                <span className="truncate text-xs text-slate-500 dark:text-slate-400">
                  {hit.domain}
                </span>
              </Command.Item>
            ))}
          </Command.Group>
        )}

        {typed !== "" && (
          <Command.Group heading={<Heading>Everywhere else</Heading>}>
            <Command.Item
              value={searchValue(typed)}
              onSelect={() => {
                go(searchRoute(typed));
              }}
              className={ROW_CLASSES}
            >
              <span className="truncate">Search for {`"${typed}"`}</span>
              <span className="text-xs text-slate-500 dark:text-slate-400">
                titles and text
              </span>
            </Command.Item>
          </Command.Group>
        )}

        {typed !== "" && hits.length === 0 && titles.isFetching && (
          <p className="px-2 py-1.5 text-sm text-slate-500 dark:text-slate-400">
            Looking for titles
          </p>
        )}
        {typed === "" && domains.length === 0 && !listing.isPending && (
          <p className="px-2 py-1.5 text-sm text-slate-500 dark:text-slate-400">
            No domains are registered on this instance yet.
          </p>
        )}
      </Command.List>
    </Command.Dialog>
  );
}
