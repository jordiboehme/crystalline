/**
 * The keyboard's way around the app.
 *
 * Cmd+K, or Ctrl+K on a keyboard without a Cmd key, anywhere in the app. What
 * it offers is the two things this instance is addressed by: the domains it
 * holds, filtered here because the whole list already arrived with the sidebar,
 * and engrams by title, which the server answers because only the index knows
 * what is in the other domains.
 *
 * Above both sits what the screen behind the palette can do right now, which
 * the screen itself registers (`commands.tsx`). It leads because it is the one
 * group that does not leave: every row under it is a jump somewhere else, and
 * the thing a reader most often opens the palette for is the thing in front of
 * them.
 *
 * What the frame registers is the exception, and it goes last. A row that is
 * identical on every screen is chrome rather than "what you are looking at",
 * and putting it first would have made the app's least specific action the
 * default Enter on the screens that offer nothing of their own.
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
import { useCallback, useEffect, useRef, useState } from "react";
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
import { usePaletteCommands } from "../commands";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
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

function actionValue(id: string): string {
  return `action:${id}`;
}

export function CommandPalette() {
  const navigate = useNavigate();
  // What the screen behind the palette can do, which is why the palette is
  // more than a way around: a write is reachable from the keyboard without
  // ever finding the button that also runs it.
  const actions = usePaletteCommands();
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

  /**
   * Shut the palette, and let go of everything a shut palette must forget.
   *
   * Every way out goes through here, which is the point: a jump closes the
   * palette from the inside and never reaches the dialog's own `onOpenChange`,
   * so a reset hanging off that callback would fire on every exit except the
   * common one and the next Cmd+K would open onto the last question. The
   * highlight is dropped with the query, because a choice made against rows
   * that are gone must not win over the new top row when the two happen to
   * line up.
   */
  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
    setTerm("");
    setChoice({ top: "", value: "" });
  }, []);

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
      if (open) {
        close();
        return;
      }
      invoker.current =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      setOpen(true);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [open, close]);

  // The focus, handed back after the dialog is gone rather than while closing
  // it: the palette traps focus for as long as it is mounted, and an invoker
  // focused a moment too early would be pulled straight back in.
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
  // The actions, matched here for the same reason the domain names are: they
  // are already in hand, and a reader holding shift is asking the same
  // question. Nothing typed matches everything, so an empty palette opens on
  // what this screen can do.
  const matching = actions.filter((command) =>
    command.title.toLowerCase().includes(typed.toLowerCase()),
  );
  // Split by who offered them: the screen's own lead the list, the frame's
  // trail it. Only the first group can take the highlight below.
  const matchingActions = matching.filter(
    (command) => command.scope === "screen",
  );
  const frameActions = matching.filter((command) => command.scope === "frame");

  // The top row, in the order they are drawn in. Enter follows the highlight,
  // and the highlight is the top row until somebody moves it off: an answer
  // landing above a highlighted row takes the highlight with it.
  const firstAction = matchingActions[0];
  const firstDomain = domains[0];
  const firstHit = hits[0];
  const top =
    firstAction !== undefined
      ? actionValue(firstAction.id)
      : firstDomain !== undefined
        ? domainValue(firstDomain.name)
        : firstHit !== undefined
          ? engramValue(firstHit)
          : typed === ""
            ? ""
            : searchValue(typed);
  const highlighted = choice.top === top ? choice.value : top;

  /** Go, and get out of the way. */
  function go(to: string): void {
    close();
    void navigate(to);
  }

  return (
    <Command.Dialog
      open={open}
      onOpenChange={(next) => {
        // Escape, or a click on the overlay. The palette itself only ever
        // opens from the shortcut, which captures the invoker as it goes.
        if (next) {
          setOpen(true);
        } else {
          close();
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
        {/*
          First, above the jumps: what the reader is already looking at is
          what they most likely opened the palette to act on, and everything
          below this leaves the screen the actions belong to.
        */}
        {matchingActions.length > 0 && (
          <Command.Group heading={<Heading>Actions</Heading>}>
            {matchingActions.map((command) => (
              <Command.Item
                key={command.id}
                value={actionValue(command.id)}
                onSelect={() => {
                  // Shut first, then act: an action that opens a dialog of its
                  // own would otherwise open it behind this one.
                  close();
                  command.run();
                }}
                className={ROW_CLASSES}
              >
                <span className="truncate">{command.title}</span>
              </Command.Item>
            ))}
          </Command.Group>
        )}

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
                // Faded when it is retired, the same way every list in this
                // app fades one: a row that jumps somewhere is exactly where
                // the fade has to hold, or a reader is sent off on a
                // deprecated engram without a word.
                className={`${ROW_CLASSES} ${
                  isRetired(hit.status) ? RETIRED_CLASS : ""
                }`}
              >
                <span className="truncate">{hit.title}</span>
                {/*
                  And the word itself, only where it changes what the row
                  means. A status on every row would be noise in a list read
                  at a glance; the fade alone says something is off without
                  saying what.
                */}
                {isRetired(hit.status) && (
                  <span className="truncate text-xs text-slate-500 dark:text-slate-400">
                    {hit.status}
                  </span>
                )}
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

        {/*
          Last, and never the highlight on an opening palette: these rows are
          the same wherever a reader is, so they are the app rather than the
          place, and the row Enter lands on should be about the place.
        */}
        {frameActions.length > 0 && (
          <Command.Group heading={<Heading>App</Heading>}>
            {frameActions.map((command) => (
              <Command.Item
                key={command.id}
                value={actionValue(command.id)}
                onSelect={() => {
                  close();
                  command.run();
                }}
                className={ROW_CLASSES}
              >
                <span className="truncate">{command.title}</span>
              </Command.Item>
            ))}
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
