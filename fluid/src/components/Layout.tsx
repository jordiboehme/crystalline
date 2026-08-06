/**
 * The frame every screen inside the app is drawn in: which domains exist down
 * the side, and search, theme and identity across the top.
 *
 * The domain listing is the one thing this fetches. It is what the sidebar is,
 * and it is also the app's answer to "what does this instance know about",
 * which is the question a person arrives with.
 *
 * Down to tablet width the frame is unchanged. Narrower than that the sidebar
 * folds behind a disclosure rather than shrinking into a column too narrow to
 * read, because a domain name that wraps three times is worse than one tap.
 */

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { DropdownMenu } from "radix-ui";
import { Link, NavLink, Outlet, useNavigate } from "react-router";

import { ApiProblem } from "../api/client";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { DomainSummary } from "../api/domains";
import { useAuth } from "../auth/AuthContext";
import { domainRoute, searchRoute } from "../paths";
import { useTheme } from "../theme/context";
import type { ThemePreference } from "../theme/context";
import { CommandPalette } from "./CommandPalette";

/** The classes every menu surface shares. */
const MENU_CLASSES =
  "z-50 min-w-48 rounded border border-slate-200 bg-white p-1 text-sm shadow-lg dark:border-slate-700 dark:bg-slate-900";

/** The classes every menu row shares. */
const ITEM_CLASSES =
  "flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 outline-none select-none data-[highlighted]:bg-slate-100 dark:data-[highlighted]:bg-slate-800";

export function Layout() {
  const [navOpen, setNavOpen] = useState(false);

  return (
    <div className="min-h-screen bg-white text-slate-900 dark:bg-slate-950 dark:text-slate-100">
      <TopBar
        navOpen={navOpen}
        onToggleNav={() => {
          setNavOpen((open) => !open);
        }}
      />
      <div className="mx-auto flex w-full max-w-350 gap-6 px-4 py-6">
        <DomainSidebar open={navOpen} />
        <main className="min-w-0 flex-1">
          <Outlet />
        </main>
      </div>
      {/*
        Once for the whole app, so the shortcut works on every screen and the
        palette outlives the screen a jump leaves behind.
      */}
      <CommandPalette />
    </div>
  );
}

function TopBar({
  navOpen,
  onToggleNav,
}: {
  navOpen: boolean;
  onToggleNav: () => void;
}) {
  const { capabilities } = useAuth();

  return (
    <header className="sticky top-0 z-40 border-b border-slate-200 bg-white/90 backdrop-blur dark:border-slate-800 dark:bg-slate-950/90">
      <div className="mx-auto flex w-full max-w-350 items-center gap-3 px-4 py-3">
        <button
          type="button"
          onClick={onToggleNav}
          aria-expanded={navOpen}
          aria-controls="domain-sidebar"
          className="rounded border border-slate-300 px-2 py-1 text-sm md:hidden dark:border-slate-700"
        >
          Domains
        </button>

        <Link
          to="/"
          className="text-lg font-semibold tracking-tight hover:opacity-80"
        >
          Fluid
        </Link>

        <SearchBox />

        {capabilities.readOnly && (
          <span className="hidden rounded bg-slate-100 px-2 py-1 text-xs text-slate-600 sm:inline dark:bg-slate-800 dark:text-slate-300">
            Read only
          </span>
        )}

        <ThemeMenu />
        <UserMenu />
      </div>
    </header>
  );
}

/** The search box. It only routes; the search screen owns the query itself. */
function SearchBox() {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");

  return (
    <form
      role="search"
      className="flex-1"
      onSubmit={(event) => {
        event.preventDefault();
        const trimmed = query.trim();
        if (trimmed === "") {
          return;
        }
        void navigate(searchRoute(trimmed));
      }}
    >
      <label htmlFor="topbar-search" className="sr-only">
        Search
      </label>
      <input
        id="topbar-search"
        type="search"
        name="q"
        value={query}
        placeholder="Search this instance"
        onChange={(event) => {
          setQuery(event.target.value);
        }}
        className="w-full rounded border border-slate-300 bg-white px-3 py-1.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-sky-500 dark:border-slate-700 dark:bg-slate-900"
      />
    </form>
  );
}

/** Light, dark, or whatever the system says. */
function ThemeMenu() {
  const { preference, resolved, choose } = useTheme();
  const options: { value: ThemePreference; label: string }[] = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger
        aria-label={`Theme: ${preference}`}
        className="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
      >
        <span aria-hidden="true">{resolved === "dark" ? "Dark" : "Light"}</span>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={6}
          className={MENU_CLASSES}
        >
          <DropdownMenu.RadioGroup
            value={preference}
            onValueChange={(value) => {
              choose(value as ThemePreference);
            }}
          >
            {options.map((option) => (
              <DropdownMenu.RadioItem
                key={option.value}
                value={option.value}
                className={ITEM_CLASSES}
              >
                <DropdownMenu.ItemIndicator>
                  <span aria-hidden="true">*</span>
                </DropdownMenu.ItemIndicator>
                {option.label}
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/**
 * Who you are, and how to stop being them.
 *
 * The anonymous viewer is named on the trigger itself rather than only inside
 * the menu: browsing without an account changes what the app will let you do,
 * so it is a fact that belongs on screen, not one you have to go looking for.
 */
function UserMenu() {
  const { user, capabilities, logout } = useAuth();
  const label = user ? user.display : "Viewing anonymously";

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger className="max-w-40 truncate rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800">
        {label}
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={6}
          className={MENU_CLASSES}
        >
          {user ? (
            <>
              <DropdownMenu.Label className="px-2 py-1.5 text-xs text-slate-500 dark:text-slate-400">
                {user.name} ({capabilities.role})
              </DropdownMenu.Label>
              <DropdownMenu.Separator className="my-1 h-px bg-slate-200 dark:bg-slate-700" />
              <DropdownMenu.Item
                className={ITEM_CLASSES}
                onSelect={() => {
                  void logout();
                }}
              >
                Log out
              </DropdownMenu.Item>
            </>
          ) : (
            <>
              <DropdownMenu.Label className="px-2 py-1.5 text-xs text-slate-500 dark:text-slate-400">
                No account on this session
              </DropdownMenu.Label>
              <DropdownMenu.Separator className="my-1 h-px bg-slate-200 dark:bg-slate-700" />
              <DropdownMenu.Item className={ITEM_CLASSES} asChild>
                <Link to="/login">Log in</Link>
              </DropdownMenu.Item>
            </>
          )}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** The domain list, which is what this instance knows about. */
function DomainSidebar({ open }: { open: boolean }) {
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });

  return (
    <nav
      id="domain-sidebar"
      aria-label="Domains"
      className={`${open ? "block" : "hidden"} w-56 shrink-0 md:block`}
    >
      <h2 className="px-2 pb-2 text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">
        Domains
      </h2>
      {listing.isPending && (
        <p className="px-2 text-sm text-slate-500 dark:text-slate-400">
          Loading domains
        </p>
      )}
      {listing.error && <SidebarProblem error={listing.error} />}
      <ul className="flex flex-col gap-0.5">
        {listing.data?.domains.map((domain) => (
          <li key={domain.name}>
            <DomainLink domain={domain} />
          </li>
        ))}
      </ul>
      {listing.data?.domains.length === 0 && (
        <p className="px-2 text-sm text-slate-500 dark:text-slate-400">
          No domains are registered on this instance yet.
        </p>
      )}
    </nav>
  );
}

function DomainLink({ domain }: { domain: DomainSummary }) {
  return (
    <NavLink
      to={domainRoute(domain.name)}
      className={({ isActive }) =>
        `flex items-baseline justify-between gap-2 rounded px-2 py-1.5 text-sm hover:bg-slate-100 dark:hover:bg-slate-800 ${
          isActive ? "bg-slate-100 font-medium dark:bg-slate-800" : ""
        }`
      }
    >
      <span className="truncate">{domain.name}</span>
      {domain.engrams !== null && (
        <span className="text-xs text-slate-500 tabular-nums dark:text-slate-400">
          {domain.engrams}
        </span>
      )}
    </NavLink>
  );
}

/**
 * A failed listing, said out loud where the listing would have been.
 *
 * Inline rather than a redirect, and that is the rule for a mid-session
 * refusal generally: a 403 here means this identity may not list domains, and
 * bouncing it to a login form it is already past would be a loop. A 401 is the
 * one exception, and it never reaches this branch - the query layer re-probes
 * on it, and the gate redirects.
 */
function SidebarProblem({ error }: { error: Error }) {
  const detail = error instanceof ApiProblem ? error.detail : error.message;
  return (
    <p
      role="alert"
      className="rounded bg-red-50 px-2 py-1.5 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
    >
      {detail}
    </p>
  );
}
