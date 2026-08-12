/**
 * The frame every screen inside the app is drawn in: which domains exist down
 * the side, and search, theme and identity across the top.
 *
 * The domain listing is the one thing this fetches. It is what the sidebar is
 * outside a domain, and it is also the app's answer to "what does this
 * instance know about", which is the question a person arrives with. Inside a
 * domain the question has changed - they are in one place and want their way
 * around it - so the sidebar changes with it and hands over to `DomainNav`,
 * which is the switcher and the folder tree.
 *
 * Down to tablet width the frame is unchanged. Narrower than that the sidebar
 * folds behind a disclosure rather than shrinking into a column too narrow to
 * read, because a domain name that wraps three times is worse than one tap.
 *
 * The keyboard lives here too, because the frame is what every screen is drawn
 * inside: the palette is mounted once, and so is the map of the keys that
 * reach it, which "?" opens from anywhere no field has the focus.
 */

import { useQuery } from "@tanstack/react-query";
import {
  Gem,
  GitBranch,
  House,
  Moon,
  PanelLeft,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Sun,
  Users as UsersIcon,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { RefObject } from "react";
import { DropdownMenu } from "radix-ui";
import {
  Link,
  NavLink,
  Outlet,
  useLocation,
  useMatch,
  useNavigate,
} from "react-router";

import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { DomainSummary } from "../api/domains";
import { useAuth } from "../auth/AuthContext";
import { useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import {
  domainRoute,
  githubSettingsRoute,
  searchRoute,
  usersRoute,
} from "../paths";
import { useTheme } from "../theme/context";
import type { ThemePreference } from "../theme/context";
import { CommandPalette } from "./CommandPalette";
import { CreateEngramDialog } from "./CreateEngramDialog";
import { DomainNav } from "./DomainNav";
import { HelpOverlay } from "./HelpOverlay";
import { ITEM_CLASSES, MENU_CLASSES } from "./menu";
import { BUTTON, Chip, FOCUS_RING, IconButton } from "./primitives";

/**
 * What the command palette's shortcut is called on this keyboard.
 *
 * The palette answers to both modifiers, so this only decides which one to
 * name, and it names the one this keyboard has.
 */
const PALETTE_HINT = /Mac|iPhone|iPad/.test(navigator.userAgent)
  ? "⌘K"
  : "Ctrl K";

/**
 * Where the sidebar's width is remembered. Its two values are "rail" and
 * "expanded"; anything else, including nothing at all, reads as expanded.
 */
const NAV_STATE_KEY = "fluid.nav";

/**
 * How wide the sidebar was last left, read once at mount.
 *
 * A browser that refuses storage - a private window with cookies off, an
 * embedded view - is not a reason to fail to draw the frame, so it gets the
 * default and keeps whatever it chooses for the session.
 */
function storedRail(): boolean {
  try {
    return localStorage.getItem(NAV_STATE_KEY) === "rail";
  } catch {
    return false;
  }
}

/** Tailwind's `md`, which is where the sidebar stops being a drawer. */
const WIDE_QUERY = "(min-width: 48rem)";

/** Whether there is a column beside the reading surface at all. */
function wideNow(): boolean {
  // An environment with no media queries to ask - jsdom, an old embedded view -
  // is answered the way the stylesheet's own default answers it, which is the
  // desktop frame; below `md` the sheet takes the sidebar over anyway.
  return (
    typeof window.matchMedia !== "function" ||
    window.matchMedia(WIDE_QUERY).matches
  );
}

/**
 * Whether the frame is wide enough to have a sidebar rather than a drawer.
 *
 * The rail is a choice about a column that only exists from `md` up. Below it
 * the sidebar is a drawer the top bar opens over the screen, and a stored rail
 * applied there would be three faults at once: a three-rem strip of icons
 * where a full-width drawer belongs, a way out of it that the stylesheet hides
 * (`hidden md:inline-flex`) so the keyboard handoff lands on nothing, and two
 * disclosures of one sidebar disagreeing about whether it is open. Gating the
 * rail on the same breakpoint the stylesheet uses closes all three, because
 * below it there is only ever the expanded drawer.
 */
function useWide(): boolean {
  // Subscribed rather than held in state: the width of the window is somebody
  // else's value, and React reads it on every render it does, so a window
  // resized between a render and the effect that would have caught it cannot
  // leave a stale answer behind.
  return useSyncExternalStore(subscribeWide, wideNow);
}

/** Watch the breakpoint, where there is anything to watch it with. */
function subscribeWide(onChange: () => void): () => void {
  if (typeof window.matchMedia !== "function") {
    return () => undefined;
  }
  const query = window.matchMedia(WIDE_QUERY);
  query.addEventListener("change", onChange);
  return () => {
    query.removeEventListener("change", onChange);
  };
}

export function Layout() {
  const { capabilities } = useAuth();
  const navigate = useNavigate();
  const [navOpen, setNavOpen] = useState(false);
  const [rail, setRail] = useState(storedRail);
  const wide = useWide();
  const [helpOpen, setHelpOpen] = useState(false);
  const mainRef = useRef<HTMLElement | null>(null);

  // A choice about the shape of the frame outlives the screen it was made on,
  // and outlives the session too: somebody who wants the reading surface as
  // wide as it goes should not have to say so again on every visit.
  const toggleRail = () => {
    setRail((was) => {
      const next = !was;
      try {
        localStorage.setItem(NAV_STATE_KEY, next ? "rail" : "expanded");
      } catch {
        // A browser that refuses storage still gets the session's choice.
      }
      return next;
    });
  };

  // What is offered on every screen, because the frame is on every screen: a
  // reader who found the palette can find everything else from inside it.
  // Registered as the frame's, so it sits under whatever the screen in front
  // of the reader offers rather than above it.
  //
  // The settings row is gated the same way its nav link is: an action a
  // session may not run is never registered, so the palette offers no door
  // that will not open.
  const commands = useMemo<PaletteCommand[]>(() => {
    const frame: PaletteCommand[] = [
      {
        id: "help",
        title: "Keyboard shortcuts",
        run: () => {
          setHelpOpen(true);
        },
      },
    ];
    if (capabilities.canAdminister) {
      frame.push({
        id: "github-settings",
        title: "GitHub settings",
        run: () => {
          void navigate(githubSettingsRoute());
        },
      });
    }
    return frame;
  }, [capabilities.canAdminister, navigate]);
  useRegisterCommands(commands, "frame");

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "?") {
        return;
      }
      // Not while somebody is writing one. A bare key is only a shortcut
      // where no field has the focus, which includes the editor's own
      // contenteditable surface as much as it does a search box.
      const target = event.target;
      if (
        target instanceof HTMLElement &&
        target.closest("input, textarea, [contenteditable=true]") !== null
      ) {
        return;
      }
      setHelpOpen(true);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  return (
    <div className="min-h-screen bg-white text-slate-900 dark:bg-slate-950 dark:text-slate-100">
      <TopBar
        navOpen={navOpen}
        onToggleNav={() => {
          setNavOpen((open) => !open);
        }}
      />
      <div className="mx-auto flex w-full max-w-350 gap-6 px-4 py-6">
        {/*
          The stored width only reaches the sidebar where there is a sidebar to
          apply it to: below `md` this is a drawer, and it is always expanded.
        */}
        <DomainSidebar
          open={navOpen}
          rail={rail && wide}
          onToggleRail={toggleRail}
        />
        <main
          ref={mainRef}
          tabIndex={-1}
          className="min-w-0 flex-1 focus:outline-none"
        >
          <Outlet />
        </main>
      </div>
      <RouteFocus target={mainRef} />
      {/*
        Once for the whole app, so the shortcut works on every screen and the
        palette outlives the screen a jump leaves behind.
      */}
      <CommandPalette />
      {/*
        And the map of the keys that drive it, one press away from anywhere.
      */}
      <HelpOverlay
        open={helpOpen}
        onClose={() => {
          setHelpOpen(false);
        }}
      />
    </div>
  );
}

/**
 * Register item 18: a route change moves focus to the new screen's main
 * region. Without this, a keyboard or screen-reader user who followed a link
 * is left focused on an element of the previous screen - or on nothing.
 * The first render is exempt: the browser's own document focus is right.
 */
function RouteFocus({ target }: { target: RefObject<HTMLElement | null> }) {
  const { pathname } = useLocation();
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    target.current?.focus();
  }, [pathname, target]);
  return null;
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
    <header className="sticky top-0 z-40 border-b border-slate-200 bg-white/90 backdrop-blur print:hidden dark:border-slate-800 dark:bg-slate-950/90">
      {/*
        A fixed row rather than one sized by its contents: the sticky offsets
        below it - the reading surface's own sticky pieces, the editor's
        toolbar - are measured against this height, so it is a number the rest
        of the frame can rely on rather than whatever the tallest control in
        here happens to be today.
      */}
      <div className="mx-auto flex h-14 w-full max-w-350 items-center gap-3 px-4">
        <IconButton
          label="Domains"
          icon={PanelLeft}
          onClick={onToggleNav}
          aria-expanded={navOpen}
          aria-controls="domain-sidebar"
          className="md:hidden"
        />

        <Link
          to="/"
          className={`flex items-center gap-1.5 rounded text-lg font-semibold tracking-tight hover:opacity-80 ${FOCUS_RING}`}
        >
          <Gem
            aria-hidden="true"
            size={18}
            strokeWidth={1.75}
            className="text-accent-600 dark:text-accent-400"
          />
          Fluid
        </Link>

        <SearchBox />

        {capabilities.readOnly && (
          <span className="hidden sm:inline">
            <Chip variant="caution">Read only</Chip>
          </span>
        )}

        {/*
          Only for the session that may use it. The screen refuses everyone
          else on its own, so this is not the guard - it is the difference
          between a frame that offers what you can do and one that offers a
          door that will not open.
        */}
        {capabilities.canAdminister && (
          <>
            <NavLink
              to={usersRoute()}
              aria-label="Users"
              title="Users"
              className={({ isActive }) =>
                `inline-flex h-8 w-8 shrink-0 items-center justify-center rounded text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING} ${
                  isActive ? "bg-slate-100 dark:bg-slate-800" : ""
                }`
              }
            >
              <UsersIcon aria-hidden="true" size={16} strokeWidth={1.75} />
            </NavLink>
            {/*
              The connection team domains are tracked with, which is why the
              glyph is a branch rather than a cog: the settings screen behind
              it is about one repository connection, not about the instance's
              preferences at large.
            */}
            <NavLink
              to={githubSettingsRoute()}
              aria-label="GitHub"
              title="GitHub"
              className={({ isActive }) =>
                `inline-flex h-8 w-8 shrink-0 items-center justify-center rounded text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING} ${
                  isActive ? "bg-slate-100 dark:bg-slate-800" : ""
                }`
              }
            >
              <GitBranch aria-hidden="true" size={16} strokeWidth={1.75} />
            </NavLink>
          </>
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
      className="mx-auto w-full max-w-xl flex-1"
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
      {/*
        The badge is the only thing that says the palette is there at all: a
        shortcut nobody is told about is a shortcut nobody presses. It sits in
        the box it is an alternative to, out of the way of the text and out of
        the way of a screen reader, which has the label above instead.
      */}
      <div className="relative">
        <input
          id="topbar-search"
          type="search"
          name="q"
          value={query}
          placeholder="Search this instance"
          onChange={(event) => {
            setQuery(event.target.value);
          }}
          className="w-full rounded border border-slate-300 bg-white py-1.5 pr-16 pl-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 dark:border-slate-700 dark:bg-slate-900"
        />
        <kbd
          aria-hidden="true"
          className="text-caption pointer-events-none absolute top-1/2 right-2 hidden -translate-y-1/2 rounded border border-slate-300 px-1 py-0.5 text-slate-500 sm:block dark:border-slate-700 dark:text-slate-400"
        >
          {PALETTE_HINT}
        </kbd>
      </div>
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
        title={`Theme: ${preference}`}
        className={`inline-flex h-8 w-8 shrink-0 items-center justify-center rounded text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING}`}
      >
        {resolved === "dark" ? (
          <Moon aria-hidden="true" size={16} strokeWidth={1.75} />
        ) : (
          <Sun aria-hidden="true" size={16} strokeWidth={1.75} />
        )}
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
      <DropdownMenu.Trigger
        className={`${BUTTON.ghost} max-w-40 truncate border border-slate-300 dark:border-slate-700`}
      >
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

/**
 * The sidebar, in whichever of its two modes the address calls for.
 *
 * The route is read with `useMatch` rather than `useParams`, which inside a
 * layout route only sees the params the layout's own pattern declares - none.
 * `useMatch` matches the whole location, so the frame knows the domain and the
 * engram the screen inside it is showing.
 *
 * It sticks under the top bar rather than scrolling away with the screen: the
 * page is the scrolling box, the header is fourteen units of it, and the
 * sidebar's own overflow starts where that ends. A long tree therefore scrolls
 * inside the sidebar while the reading surface scrolls the page, which is what
 * keeps the way around a domain reachable from the bottom of a long engram.
 * The half-unit of side padding that comes with it is the focus ring's: a box
 * that scrolls on one axis clips the other one too, and the rows inside fill
 * this column edge to edge, so without it a focused row's ring would be shaved
 * off down both sides.
 */
function DomainSidebar({
  open,
  rail,
  onToggleRail,
}: {
  open: boolean;
  rail: boolean;
  onToggleRail: () => void;
}) {
  const navigate = useNavigate();
  const { capabilities } = useAuth();
  const [creating, setCreating] = useState(false);
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });

  // Folding the sidebar unmounts the control that folded it, so the keyboard
  // is handed to the one that took its place - the same spot on screen, the
  // opposite verb. Without this, focus lands on the document and a reader is
  // back at the top of the page. Only after a press: a mount that read the
  // stored preference must not steal focus from wherever the reader is.
  const toggleRef = useRef<HTMLButtonElement | null>(null);
  const pressed = useRef(false);
  useEffect(() => {
    if (!pressed.current) {
      return;
    }
    pressed.current = false;
    toggleRef.current?.focus();
  }, [rail]);
  const fold = () => {
    pressed.current = true;
    onToggleRail();
  };

  const match = useMatch("/d/:domain/*");
  const domain = match?.params.domain ?? "";
  // The splat holds whatever follows the domain: `e/<permalink>` on an engram
  // screen, `edit/<permalink>` in the editor, the empty string on the
  // domain's own home. Both prefixed forms carry the same permalink, so the
  // sidebar highlights the right tree row and offers its own launcher on
  // either screen, and only the domain's own home - where the permalink is
  // truly absent - is treated as having none.
  const rest = match?.params["*"] ?? "";
  const permalink = rest.startsWith("e/")
    ? rest.slice(2)
    : rest.startsWith("edit/")
      ? rest.slice(5)
      : "";
  // The MANIFEST page and its editor both live outside the splat's engram
  // shapes above - `manifest` is its own reserved segment (`routes.tsx`),
  // never a permalink the splat would otherwise swallow - so this is the
  // you-are-here cue the pinned tree row needs, the same thing `permalink`
  // already gives the ordinary rows.
  const onManifest = rest === "manifest" || rest.startsWith("manifest/");

  return (
    <nav
      id="domain-sidebar"
      aria-label={domain === "" ? "Domains" : `Domain ${domain}`}
      className={`${open ? "block" : "hidden"} ${
        rail ? "w-12" : "w-56"
      } shrink-0 print:hidden md:sticky md:top-14 md:block md:max-h-[calc(100vh-3.5rem)] md:self-start md:overflow-y-auto md:px-0.5`}
    >
      {rail ? (
        /*
          Folded: the two things somebody in the middle of reading still needs
          from the frame, and the way back out of the fold. Names rather than
          bare glyphs, because an icon column is exactly where a keyboard or a
          screen reader has the least to go on.
        */
        <div className="flex flex-col items-center gap-1 py-1">
          <IconButton
            ref={toggleRef}
            label="Expand the sidebar"
            icon={PanelLeftOpen}
            aria-expanded={false}
            aria-controls="domain-sidebar"
            onClick={fold}
          />
          <IconButton
            label="All domains"
            icon={House}
            onClick={() => {
              void navigate("/");
            }}
          />
          {/*
            The same condition `DomainNav` puts on its own launcher, for the
            same reason: the domain's home screen carries one beside its
            heading, prefilled with the folder being browsed, and the frame
            offering a second one would be two controls for one act.
          */}
          {permalink !== "" && capabilities.canWrite && (
            <IconButton
              label="New engram"
              icon={Plus}
              onClick={() => {
                setCreating(true);
              }}
            />
          )}
        </div>
      ) : (
        <>
          <div className="flex justify-end pb-1">
            {/*
              Only where there is a rail to fold into: below the medium
              breakpoint the sidebar is a drawer the top bar's own disclosure
              opens and shuts, and a second control for the same column would
              be one too many.
            */}
            <IconButton
              ref={toggleRef}
              label="Collapse the sidebar"
              icon={PanelLeftClose}
              aria-expanded={true}
              aria-controls="domain-sidebar"
              onClick={fold}
              className="hidden md:inline-flex"
            />
          </div>
          {domain === "" ? (
            <DomainList
              domains={listing.data?.domains}
              pending={listing.isPending}
              error={listing.error}
            />
          ) : (
            <>
              {/*
                A listing that failed is said here as it is in the flat mode.
                The switcher is drawn either way: it names the domain the
                reader is in from the address, so it works even when nothing
                could be listed to switch to.
              */}
              {listing.error && <SidebarProblem error={listing.error} />}
              <DomainNav
                domain={domain}
                permalink={permalink}
                onManifest={onManifest}
                domains={listing.data?.domains ?? []}
              />
            </>
          )}
        </>
      )}
      {creating && (
        <CreateEngramDialog
          domain={domain}
          initialFolder=""
          onClose={() => {
            setCreating(false);
          }}
        />
      )}
    </nav>
  );
}

/** Every domain this instance holds: the sidebar outside any one of them. */
function DomainList({
  domains,
  pending,
  error,
}: {
  domains: DomainSummary[] | undefined;
  pending: boolean;
  error: Error | null;
}) {
  return (
    <>
      <h2 className="text-caption px-2 pb-2 font-semibold text-slate-500 dark:text-slate-400">
        Domains
      </h2>
      {pending && (
        <p className="px-2 text-sm text-slate-500 dark:text-slate-400">
          Loading domains
        </p>
      )}
      {error && <SidebarProblem error={error} />}
      <ul className="flex flex-col gap-0.5">
        {domains?.map((domain) => (
          <li key={domain.name}>
            <DomainLink domain={domain} />
          </li>
        ))}
      </ul>
      {domains?.length === 0 && (
        <p className="px-2 text-sm text-slate-500 dark:text-slate-400">
          No domains are registered on this instance yet.
        </p>
      )}
    </>
  );
}

function DomainLink({ domain }: { domain: DomainSummary }) {
  return (
    <NavLink
      to={domainRoute(domain.name)}
      className={({ isActive }) =>
        `flex items-baseline justify-between gap-2 rounded px-2 py-1.5 text-sm hover:bg-slate-100 dark:hover:bg-slate-800 ${FOCUS_RING} ${
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
  const detail = problemDetail(error);
  return (
    <p
      role="alert"
      className="rounded bg-red-50 px-2 py-1.5 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
    >
      {detail}
    </p>
  );
}
