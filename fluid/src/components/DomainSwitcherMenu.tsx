/**
 * What the domain switcher's menu holds (spec 4.1): a filter field, "All
 * domains", every domain with its chip and engram count and a check on the
 * current one, then "New domain" for an admin.
 *
 * Lazy: only the trigger is in the entry (`DomainSwitcher.tsx`), and this
 * body loads the first time somebody opens or points at it.
 *
 * The list is the domain listing plus the domains a person reaches only
 * through their Reflection (a contributor's), which the listing leaves out.
 *
 * The filter is a plain field inside the menu, not a combobox: the rows stay
 * menu items, so the arrow keys, Enter and Escape behave as in every other
 * menu here. ArrowDown hands the keyboard from the field to the first row.
 */

import { useQuery } from "@tanstack/react-query";
import { Check, House, Plus, Search } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { useEffect, useRef, useState } from "react";
import type { ReactElement } from "react";
import { Link } from "react-router";

import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import {
  REFLECTION_DOMAINS_KEY,
  fetchReflectionDomains,
} from "../api/reflectionDomains";
import { useAuth } from "../auth/AuthContext";
import { mayWriteIn } from "../domainWritable";
import { domainRoute, reflectionRoute } from "../paths";
import { Loading } from "./Loading";
import { MENU_SEPARATOR } from "./menu";
import { Chip, FOCUS_RING } from "./primitives";

/** One row of the menu, whichever listing it came from. */
interface Row {
  name: string;
  to: string;
  engrams: number | null;
  chip: string | null;
}

const ROW =
  "flex h-[34px] cursor-pointer items-center gap-2.5 rounded-[7px] px-2.5 text-ink-2 outline-none select-none data-[highlighted]:bg-sunken";

export default function DomainSwitcherMenu({
  domain,
  onCreateDomain,
}: {
  /** The domain the route is inside, or "" outside every domain. */
  domain: string;
  /** Register a domain, or null for a session that may not. */
  onCreateDomain: (() => void) | null;
}): ReactElement {
  const { capabilities } = useAuth();
  const [filter, setFilter] = useState("");
  const field = useRef<HTMLInputElement>(null);
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const reflections = useQuery({
    queryKey: REFLECTION_DOMAINS_KEY,
    queryFn: fetchReflectionDomains,
  });

  // The menu focuses itself once it is mounted, after this component's own
  // effects have run, so the field takes the keyboard one tick later.
  useEffect(() => {
    const timer = setTimeout(() => field.current?.focus());
    return () => {
      clearTimeout(timer);
    };
  }, []);

  const reflecting = new Set(
    (reflections.data ?? [])
      .filter((row) => row.state !== "off")
      .map((row) => row.domain),
  );
  // Whether this person writes anywhere at all, by role or, on a domain
  // GitHub governs, by their GitHub role.
  const writesSomewhere =
    capabilities.canWrite ||
    (listing.data?.domains ?? []).some((entry) =>
      mayWriteIn(capabilities, entry),
    );
  const rows: Row[] = (listing.data?.domains ?? []).map((entry) => ({
    name: entry.name,
    to: domainRoute(entry.name),
    engrams: entry.engrams,
    // About this person, not the domain's lock (see domainWritable.ts), and
    // only where they write elsewhere: for a viewer it would be every row.
    chip: entry.private
      ? "private"
      : writesSomewhere && entry.writeBlocked
        ? "view only"
        : reflecting.has(entry.name)
          ? "Reflection"
          : null,
  }));
  for (const row of reflections.data ?? []) {
    if (!row.engramsVisible && !rows.some((r) => r.name === row.domain)) {
      rows.push({
        name: row.domain,
        to: reflectionRoute(row.domain),
        engrams: null,
        chip: "Reflection",
      });
    }
  }
  rows.sort((a, b) => a.name.localeCompare(b.name));
  const needle = filter.trim().toLowerCase();
  const shown = rows.filter((row) => row.name.toLowerCase().includes(needle));

  return (
    <>
      <div className="relative px-1 pt-1 pb-2">
        <Search
          aria-hidden="true"
          size={15}
          strokeWidth={1.75}
          className="pointer-events-none absolute top-[calc(50%-2px)] left-3.5 -translate-y-1/2 text-faint"
        />
        <input
          ref={field}
          type="search"
          aria-label="Find a domain"
          placeholder="Find a domain"
          value={filter}
          onChange={(event) => {
            setFilter(event.target.value);
          }}
          onKeyDown={(event) => {
            const items = event.currentTarget
              .closest("[role=menu]")
              ?.querySelectorAll<HTMLElement>("[role=menuitem]");
            if (event.key === "ArrowDown") {
              event.preventDefault();
              items?.[0]?.focus();
            } else if (event.key === "Enter" && shown.length === 1) {
              // One match left: Enter opens it, as a click on its row would.
              event.preventDefault();
              items?.[1]?.click();
            } else if (event.key !== "Escape") {
              // The menu's typeahead would take every letter otherwise.
              event.stopPropagation();
            }
          }}
          className={`h-8 w-full rounded-lg border border-hair-strong bg-raised pr-2.5 pl-8 text-[13px] text-ink outline-none placeholder:text-faint ${FOCUS_RING}`}
        />
      </div>
      <DropdownMenu.Item
        asChild
        className={ROW}
        onKeyDown={(event) => {
          // Up from the first row goes back to the field above it.
          if (event.key === "ArrowUp") {
            event.preventDefault();
            field.current?.focus();
          }
        }}
      >
        <Link to="/">
          <House
            aria-hidden="true"
            size={16}
            strokeWidth={1.75}
            className="text-muted"
          />
          All domains
          <span className="ml-auto text-xs text-faint tabular-nums">
            {rows.length}
          </span>
        </Link>
      </DropdownMenu.Item>
      <DropdownMenu.Separator className={MENU_SEPARATOR} />
      <DropdownMenu.Label className="px-2.5 pt-2 pb-1.5 text-[11.5px] font-semibold tracking-[0.04em] text-faint uppercase">
        Domains
      </DropdownMenu.Label>
      {shown.length === 0 &&
        (listing.isPending ? (
          <Loading compact label="Loading domains" className="px-2.5 py-2" />
        ) : (
          <p className="px-2.5 py-2 text-muted">No domain matches.</p>
        ))}
      {shown.map((row) => (
        <DropdownMenu.Item key={row.name} asChild className={ROW}>
          <Link
            to={row.to}
            aria-current={row.name === domain ? "true" : undefined}
          >
            <span className="w-4 shrink-0">
              {row.name === domain && (
                <Check
                  aria-hidden="true"
                  size={16}
                  strokeWidth={1.75}
                  className="text-muted"
                />
              )}
            </span>
            <span className="truncate">{row.name}</span>
            {row.name === domain && <span className="sr-only">(current)</span>}
            <span className="ml-auto flex shrink-0 items-center gap-1.5 text-xs text-faint tabular-nums">
              {row.chip !== null && (
                <Chip variant={row.chip === "view only" ? "neutral" : "accent"}>
                  {row.chip}
                </Chip>
              )}
              {row.engrams}
            </span>
          </Link>
        </DropdownMenu.Item>
      ))}
      {onCreateDomain !== null && (
        <>
          <DropdownMenu.Separator className={MENU_SEPARATOR} />
          <DropdownMenu.Item className={ROW} onSelect={onCreateDomain}>
            <Plus
              aria-hidden="true"
              size={16}
              strokeWidth={1.75}
              className="text-muted"
            />
            New domain
          </DropdownMenu.Item>
        </>
      )}
    </>
  );
}
