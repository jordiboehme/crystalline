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
 * reach it, which "?" opens from anywhere no field has the focus. The other
 * bare key is "\\", which is the frame's other choice about its own shape:
 * how wide what you came to read is allowed to be.
 */

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import {
  House,
  PanelLeft,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Search,
  Share2,
  X,
} from "lucide-react";
import {
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ReactNode, RefObject } from "react";
import {
  Link,
  NavLink,
  Outlet,
  useLocation,
  useMatch,
  useNavigate,
} from "react-router";

import {
  GITHUB_STATUS_KEY,
  SYNC_SUMMARY_KEY,
  SYNC_SUMMARY_STALE_MS,
  fetchGithubStatus,
  fetchSyncStatus,
  fetchSyncSummary,
  syncStatusKey,
} from "../api/admin";
import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import {
  REFLECTION_DOMAINS_KEY,
  fetchReflectionDomains,
} from "../api/reflectionDomains";
import type { DomainListing, DomainSummary } from "../api/domains";
import { useAuth } from "../auth/AuthContext";
import { useDomainWritable } from "../domainWritable";
import { openWindow } from "../base";
import { useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import { ChangeStreamProvider } from "../events/ChangeStreamProvider";
import {
  LAYOUT_TEXT_KEY,
  LayoutTextContext,
  storedLargeText,
} from "../layoutText";
import { lazyChunk } from "../lazyChunk";
import type { LayoutText } from "../layoutText";
import {
  LAYOUT_WIDTH_KEY,
  LayoutWidthContext,
  storedFullWidth,
  useFullWidth,
} from "../layoutWidth";
import type { LayoutWidth } from "../layoutWidth";
import {
  ENGRAMS_ORDER_KEY,
  EngramsOrderContext,
  storedEngramsOrder,
} from "../engramsOrder";
import type { EngramsOrder, EngramsOrderChoice } from "../engramsOrder";
import { HANDBOOK_URL } from "../handbook";
import {
  domainRoute,
  githubSettingsRoute,
  reflectionPathOf,
  searchRoute,
} from "../paths";
import { PREFETCH } from "../prefetch";
import { useWide } from "../useWide";
import { GithubAccessBanner } from "./GithubAccessBanner";
import { OutlineSlotContext } from "./outlineSlot";
import { CommandPalette, openPalette } from "./CommandPalette";
import { CreateDomainDialog } from "./CreateDomainDialog";
import { CreateEngramDialog } from "./CreateEngramDialog";
import { DomainNav } from "./DomainNav";
import { AvatarMenu } from "./AvatarMenu";
import { DomainSwitcher } from "./DomainSwitcher";
import { HelpOverlay } from "./HelpOverlay";
import { JoinedDraftBar } from "./JoinedDraftBar";
import { LazyBoundary } from "./LazyBoundary";
import {
  ReflectionModeSwitch,
  switchHome,
  useReflectionStatus,
} from "./ReflectionModeSwitch";
import { ShareDialog } from "./ShareDialog";
import { SharePickerDialog } from "./SharePickerDialog";
import { ShatterGem } from "./ShatterGem";
import { ownedPhrase, shareBadgeCount } from "./changes";
import { Loading, ReadyFlash } from "./Loading";
import { Chip, FOCUS_RING, IconButton, SIDE, Tooltip } from "./primitives";
import { SidebarStateContext, useSidebarState } from "./sidebarState";
import type { SidebarCreate, SidebarState } from "./sidebarState";

const ReflectionNav = lazyChunk(() => import("./ReflectionNav"));
const NewReflectionDialog = lazyChunk(() =>
  import("./NewReflectionDialog").then((m) => ({
    default: m.NewReflectionDialog,
  })),
);

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

/**
 * Whether the frame offers to share, what a share would go to, and why it
 * will not.
 *
 * Sharing is the one act in this app that is about the whole instance and about
 * one domain at the same time, so the answer is two answers: inside a team
 * domain the domain being read is the answer, and everywhere else the question
 * is which of them, which is the picker.
 */
interface ShareAction {
  /** Whether the frame draws the action at all. */
  visible: boolean;
  /**
   * The domain a share would go to, or null when which one is still to be
   * decided - which is what opens the picker rather than the share dialog.
   */
  domain: string | null;
  /** Whether there is anything to share right now. */
  enabled: boolean;
  /** Why it will not act, for the tooltip; null while it will. */
  reason: string | null;
  /**
   * How much is waiting, for the badge on the face of the button: this
   * session's own share of it where the server said, everything waiting
   * otherwise. Null while there is nothing to draw a number for.
   */
  count: number | null;
  /**
   * What is waiting, said in words, for the tooltip of a button that WILL act -
   * "2 of 5 unshared changes are yours". Null when the report carried no owned
   * count, which leaves the tooltip exactly as it always read.
   */
  waiting: string | null;
}

/**
 * What the action says while the read behind it is still out.
 *
 * Not "nothing to share": that is a verdict, and a verdict handed down before
 * the answer arrives is one that may be about to be wrong.
 */
const LOOKING = "Looking for changes to share";

/** The action as it is off: nothing offered, and nothing to explain. */
const NO_SHARE: ShareAction = {
  visible: false,
  domain: null,
  enabled: false,
  reason: null,
  count: null,
  waiting: null,
};

/**
 * How long a live origin probe the FRAME fires stays fresh, in milliseconds.
 *
 * The same window the summary carries, and deliberately the same number: both
 * of these reads reach GitHub, both are mounted on every screen because the
 * frame is, and one freshness policy for the frame's probes is easier to reason
 * about than two. A screen that wants a fresher answer asks for one - the sync
 * and proposals cards keep their own options on the same key, which react-query
 * resolves per observer.
 */
const PROBE_STALE_MS = SYNC_SUMMARY_STALE_MS;

/**
 * The three reads behind the share action, and what they add up to.
 *
 * Called from the frame and from the top bar both, which costs one extra
 * observer per query and not one extra request: react-query hands the second
 * subscriber the first one's result, which is the same thing the proposals card
 * does with the sync card's status. The alternative - deciding in the frame and
 * passing the verdict down - would put the top bar's own face in the frame's
 * hands for no gain, and the palette row needs the same verdict either way.
 *
 * Every read is gated on the one above it, so a session that is offered nothing
 * asks for nothing: a non-admin never reads the connection, an instance with
 * GitHub off never reads a status, and the instance-wide summary - which probes
 * every origin at once - is only ever asked when the domain being read cannot
 * answer for itself.
 *
 * The two reads that reach GitHub also step out of the app's refetch-on-focus
 * default. That default is right for content somebody may have edited in
 * another tab and wrong for a probe: coming back to a tab would fire a live
 * origin check per team domain, each under that domain's origin lock, to
 * refresh a button's face. The stale window above IS the freshness policy here,
 * and focus must not be a way around it.
 */
function useShareAction(): ShareAction {
  const { capabilities } = useAuth();
  const admin = capabilities.canAdminister;
  // Who the share surfaces are open to, which is not the same question as who
  // administers the instance: `can_share` is an admin always and an editor
  // where `github.share_identity` is `personal`, and it is the server's own
  // verdict - the very predicate the share routes gate on - rather than a rule
  // this side re-derives. The domain home already draws its share cards on it.
  const sharer = capabilities.canShare;

  // Under the settings screen's own key, so an admin who has been there pays
  // nothing for it here. Admin-only, because the route is: an editor who may
  // share cannot read the instance's GitHub settings and never asks.
  const connection = useQuery({
    queryKey: GITHUB_STATUS_KEY,
    queryFn: fetchGithubStatus,
    enabled: admin,
  });
  // Hidden while the answer is still out rather than drawn and withdrawn: a
  // control that appears a beat late is better than one that appears and then
  // admits it was never there. Read-only rules it out on its own - a share
  // writes to the origin, and this instance refuses writes.
  //
  // The readiness half is asked of whichever source this session can reach.
  // An admin has the settings probe and waits for it. An editor who may share
  // has no such route, and `can_share` is already the answer it would give:
  // the server only says yes where sharing is turned on and this session's
  // role reaches it. The reads below carry their own connection verdict, and
  // in personal mode the instance credential is not what an editor shares
  // with anyway, so there is nothing here for a missing one to gate.
  const visible =
    sharer &&
    !capabilities.readOnly &&
    (!admin || connection.data?.enabled === true);

  // `useMatch` rather than `useParams`, for the reason `DomainSidebar` spells
  // out: inside a layout route the params are the layout's own, which are none.
  const match = useMatch("/d/:domain/*");
  const routeDomain = match?.params.domain ?? "";

  // The card's own key and fetcher, and the same refusal to retry a decided
  // answer. On the domain's home screen that costs nothing at all, because the
  // sync and proposals cards are already asking; on every other screen under
  // `/d/<domain>` - an engram, the editor, the tree - the frame is the only
  // one asking, and this is a live origin probe. Hence the window and the
  // focus rule below: the cards keep their own options on the same key, which
  // react-query resolves per observer rather than per query.
  const status = useQuery({
    queryKey: syncStatusKey(routeDomain),
    queryFn: () => fetchSyncStatus(routeDomain),
    retry: false,
    staleTime: PROBE_STALE_MS,
    refetchOnWindowFocus: false,
    enabled: visible && routeDomain !== "",
  });
  // A domain with no origin answers 404, and that is not a dead end: the
  // question widens to the whole instance, which is the one somebody standing
  // in a local domain and pressing Share would have meant anyway.
  const instanceWide = routeDomain === "" || status.isError;

  const summary = useQuery({
    queryKey: SYNC_SUMMARY_KEY,
    queryFn: fetchSyncSummary,
    staleTime: PROBE_STALE_MS,
    refetchOnWindowFocus: false,
    // Not without a credential to read the origins with: the route reports a
    // missing connection rather than refusing over it, so this would be a
    // round trip whose every count is local state alone, to fill in a face
    // that is already saying the one thing a reader can act on.
    enabled: visible && instanceWide && connection.data?.connected !== false,
  });

  if (!visible) {
    return NO_SHARE;
  }
  // Said before anything about what is waiting, because it is the one reason
  // here that a reader can do something about, and because the counts behind
  // the other reasons cannot be trusted without a credential to read them.
  if (connection.data?.connected === false) {
    return {
      ...NO_SHARE,
      visible: true,
      reason: "Connect GitHub first",
    };
  }
  if (!instanceWide) {
    const total = status.data?.localChanges ?? 0;
    const owned = status.data?.ownedChanges ?? null;
    return {
      visible: true,
      domain: routeDomain,
      enabled: total > 0,
      reason:
        total > 0
          ? null
          : status.data === undefined
            ? LOOKING
            : `Nothing to share in ${routeDomain}`,
      count: total > 0 ? shareBadgeCount(owned, total) : null,
      waiting: total > 0 ? ownedPhrase(owned, total, "unshared changes") : null,
    };
  }
  const waiting = (summary.data?.domains ?? []).filter(
    (entry) => entry.localChanges > 0,
  );
  const total = waiting.reduce((sum, entry) => sum + entry.localChanges, 0);
  // The pairing is summed over the domains that ANSWERED with an owned count,
  // both halves of it: a row that said nothing about ownership contributes to
  // neither the numerator nor the denominator. Summing its changes into the
  // denominator alone would say "2 of 8 are yours" about eight changes when
  // three of them were never attributed to anybody - a claim the report never
  // made. The badge keeps counting everything that waits (that is what the
  // button acts on); only the sentence narrows to what is known.
  const answered = waiting.filter((entry) => entry.ownedChanges !== null);
  // Null when nothing answered: an instance whose report says nothing about
  // ownership must not be summed into a confident zero.
  const owned = answered.length
    ? answered.reduce((sum, entry) => sum + (entry.ownedChanges ?? 0), 0)
    : null;
  const known = answered.reduce((sum, entry) => sum + entry.localChanges, 0);
  return {
    visible: true,
    domain: null,
    enabled: waiting.length > 0,
    reason:
      waiting.length > 0
        ? null
        : summary.isError
          ? // A read that failed is not a read still out: retries have been
            // spent and nothing else is coming, so LOOKING would be a
            // permanent lie. The server's own sentence is better than a house
            // one - the reachable case is GitHub switched off, which says so
            // and says where to turn it on.
            problemDetail(summary.error)
          : summary.data === undefined
            ? LOOKING
            : "Nothing to share",
    count: waiting.length > 0 ? shareBadgeCount(owned, total) : null,
    waiting:
      waiting.length > 0 ? ownedPhrase(owned, known, "unshared changes") : null,
  };
}

/**
 * The local name a listing resolves `seg` to, or null when there is nothing
 * to redirect to: `seg` is already a local name, or it is not exactly one
 * domain's canonical name or alias.
 */
function resolvedLocalName(listing: DomainListing, seg: string): string | null {
  if (listing.domains.some((entry) => entry.name === seg)) {
    return null;
  }
  const matches = listing.domains.filter(
    (entry) => entry.canonicalName === seg || entry.aliases.includes(seg),
  );
  const [only] = matches;
  return only !== undefined && matches.length === 1 ? only.name : null;
}

/**
 * A domain segment in the current address that used to be a domain's local
 * name here, sent to the local name it answers to now.
 *
 * Renaming a domain keeps every former name - the old local name, the
 * canonical one, every alias - resolving on the server, but every route in
 * this app reads its `:domain` segment as a local name and nothing else.
 * Read once, above every screen the pattern matches (`DomainHome`, the
 * engram screen, both editors, the graph), rather than once per screen: all
 * of them need only the listing the sidebar already fetches under the same
 * key, so the first look at it costs no request of its own.
 *
 * Waits for that listing to answer before it decides anything, and only ever
 * redirects when the segment resolves to EXACTLY one domain: zero or several
 * candidates leave the address alone, and the screen underneath says what it
 * always says about it - a wrong address, or an ambiguity nothing here is
 * positioned to resolve. Never on the server, and never before an authorized
 * read has landed, so a domain a caller may not see is never disclosed by a
 * redirect firing ahead of that read.
 *
 * A cache-level match is never enough to act on by itself, though: this
 * client's own copy of the listing can be stale (nothing here forces a
 * refetch on every route change), and a stale copy that still carries a
 * domain's OLD local name as its canonical one would resolve a fresh link to
 * that domain's NEW local name straight back to the address a rename already
 * retired - sending the reader backward instead of forward. So a cache-level
 * match only earns a confirming `fetchQuery` (a genuine round trip, not
 * answered from the cache alone); only if that fresh read still resolves the
 * segment the same way does this navigate at all. A segment with no
 * cache-level match costs nothing extra: the ordinary case, a wrong address a
 * fresh read would not fix, is not taxed with a request it does not need.
 *
 * The confirming fetch's `confirming` guard - which segment a fetch has
 * already been sent for, so a stubborn stale value does not queue a fresh
 * one on every unrelated re-render - is cleared once that fetch settles,
 * whichever way it settles: a fresh read that resolves to "nothing to
 * redirect to" clears it so a LATER change to the listing gets its own
 * fresh try, and a REJECTED fetch (a dropped connection, a 5xx) clears it
 * the same way rather than leaving this segment's redirect silently and
 * permanently disabled for the rest of the session over one transient
 * failure. Nothing here retries on its own, though: the next attempt only
 * ever comes from the next render this effect would have run for anyway
 * (the listing refetching for an unrelated reason, a window focus, this
 * segment's own rename actually landing), so a fetch that keeps failing
 * costs one request per such change, never a loop.
 */
function useDomainNameRedirect(): void {
  const match = useMatch("/d/:domain/*");
  const seg = match?.params.domain ?? null;
  const pathnameBase = match?.pathnameBase ?? null;
  const domains = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const queryClient = useQueryClient();
  const location = useLocation();
  const navigate = useNavigate();
  // Which segment a confirming fetch has already been sent for, so a
  // stubborn stale value sitting in `domains.data` (nothing here changes it)
  // does not queue a fresh round trip on every unrelated re-render.
  const confirming = useRef<string | null>(null);

  useEffect(() => {
    if (seg === null || pathnameBase === null || domains.data === undefined) {
      return;
    }
    if (confirming.current === seg) {
      return;
    }
    if (resolvedLocalName(domains.data, seg) === null) {
      return;
    }
    confirming.current = seg;
    let cancelled = false;
    void queryClient
      .fetchQuery({ queryKey: DOMAINS_QUERY_KEY, queryFn: fetchDomains })
      .then((fresh: DomainListing) => {
        if (cancelled) {
          return;
        }
        const local = resolvedLocalName(fresh, seg);
        if (local === null) {
          // Confirmed there is nothing to redirect to, off a fresh read -
          // cleared rather than left set, so a LATER change to the listing
          // (the rename this segment is waiting on actually landing, a
          // window-focus refetch, anything) earns this segment a fresh
          // confirming read of its own instead of being skipped forever.
          confirming.current = null;
          return;
        }
        // The tail is copied out of the raw pathname rather than rebuilt
        // from the splat's decoded params: a permalink segment already
        // carries its own encoding, and re-encoding a decoded value can
        // change it.
        const tail = location.pathname.slice(pathnameBase.length);
        void navigate(
          `${domainRoute(local)}${tail}${location.search}${location.hash}`,
          { replace: true, state: location.state as unknown },
        );
      })
      .catch(() => {
        if (cancelled) {
          return;
        }
        // A transient failure (a dropped connection, a 5xx) must not
        // permanently disable a legitimate redirect for the rest of the
        // session: cleared the same way an "nothing to redirect to" answer
        // is, so a later change to `domains.data` gets its own fresh try
        // rather than this segment being silently given up on. Never
        // retried from in here directly - only ever from the next render
        // this effect would have run for anyway - so a fetch that keeps
        // failing costs one request per change to the listing, not a loop.
        confirming.current = null;
      });
    return () => {
      cancelled = true;
    };
  }, [
    seg,
    pathnameBase,
    domains.data,
    // Alongside `domains.data`, not instead of it: react-query keeps `.data`
    // at the SAME object reference across a refetch whose answer is
    // structurally identical to what was already cached ("structural
    // sharing"), so a retry after a transient failure - the exact case
    // `confirming` being cleared above exists for - could refetch
    // successfully and still never earn this effect another run if `.data`
    // were the only signal watched. This timestamp changes on every
    // successful fetch regardless of whether the content did.
    domains.dataUpdatedAt,
    location,
    navigate,
    queryClient,
  ]);
}

export function Layout() {
  const { capabilities } = useAuth();
  const navigate = useNavigate();
  // A saved link, a bookmark or a search result may still carry a domain's
  // old address; every screen under one keeps reaching it, so this fires
  // once, above all of them, rather than in each screen that has a domain
  // segment in its own route.
  useDomainNameRedirect();
  const [navOpen, setNavOpen] = useState(false);
  // The phone drawer lies over the page, so following a link out of it shuts
  // it: the reader came for the page the link opens, not for the drawer.
  // Adjusted while rendering, the way React documents state that follows a
  // value, rather than in an effect.
  const location = useLocation();
  const at = `${location.pathname}${location.search}`;
  const [navAt, setNavAt] = useState(at);
  if (navAt !== at) {
    setNavAt(at);
    setNavOpen(false);
  }
  // The bar's drawer toggle, where the keyboard goes back to when the drawer
  // is shut from inside (its close button, Escape).
  const navToggleRef = useRef<HTMLButtonElement | null>(null);
  const [rail, setRail] = useState(storedRail);
  const [fullWidth, setFullWidth] = useState(storedFullWidth);
  const [largeText, setLargeText] = useState(storedLargeText);
  const [engramsOrder, setEngramsOrderState] = useState(storedEngramsOrder);
  const wide = useWide();
  // A window that grows past the breakpoint has no drawer, so it is shut for
  // good rather than left to come back when the window narrows again.
  if (wide && navOpen) {
    setNavOpen(false);
  }
  const [helpOpen, setHelpOpen] = useState(false);
  // The Help button, when it is what opened the shortcut map: a menu item has
  // unmounted by the time the dialog closes, so focus would be lost to <body>.
  const [helpReturn, setHelpReturn] = useState<HTMLElement | null>(null);
  // What the sidebar keeps outside its own rows (see `sidebarState.ts`): the
  // folders opened by hand and the create dialog it asked for, which the
  // frame draws so neither is lost when the drawer shuts or the window
  // crosses the breakpoint.
  const [folders, setFolders] = useState<Record<string, boolean>>({});
  const [sidebarCreate, setSidebarCreate] = useState<SidebarCreate | null>(
    null,
  );
  // The outline a long page hands the phone drawer (see `outlineSlot.ts`).
  const [outlineNode, setOutlineNode] = useState<ReactNode>(null);
  const closeDrawer = useCallback(() => {
    setNavOpen(false);
  }, []);
  const outlineSlot = useMemo(
    () => ({ node: outlineNode, set: setOutlineNode, close: closeDrawer }),
    [outlineNode, closeDrawer],
  );
  const sidebarState = useMemo<SidebarState>(
    () => ({
      folders,
      setFolder: (key, open) => {
        setFolders((was) =>
          was[key] === open ? was : { ...was, [key]: open },
        );
      },
      create: setSidebarCreate,
    }),
    [folders],
  );
  // Registering a domain is the frame's own act rather than any screen's: it
  // is what the sidebar lists, and the sidebar is on every screen.
  const [creatingDomain, setCreatingDomain] = useState(false);
  // Sharing is the frame's for the same reason, and it is two states rather
  // than one: the domain a share is aimed at, and the question of which domain
  // that is. Both dialogs are mounted here, because both ways in - the top
  // bar's button and the palette row - are the frame's.
  const [shareDomain, setShareDomain] = useState<string | null>(null);
  const [pickingShare, setPickingShare] = useState(false);
  const share = useShareAction();
  const mainRef = useRef<HTMLElement | null>(null);

  // Which of the two doors the one act opens: straight into the share dialog
  // where the domain is already known, and into the picker where it is not.
  const openShare = useCallback(
    (domain: string | null) => {
      if (domain === null) {
        setPickingShare(true);
      } else {
        setShareDomain(domain);
      }
    },
    [setPickingShare, setShareDomain],
  );

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

  // The same choice about the frame, made about the content rather than about
  // the column beside it, and kept the same way. Memoized where `toggleRail`
  // is not: this one is read by the key handler's effect and by the palette
  // registration, both of which key off identity, so a fresh function every
  // render would re-subscribe the window and re-register the row forever.
  const toggleFullWidth = useCallback(() => {
    setFullWidth((was) => {
      const next = !was;
      try {
        localStorage.setItem(LAYOUT_WIDTH_KEY, next ? "full" : "reading");
      } catch {
        // A browser that refuses storage still gets the session's choice.
      }
      return next;
    });
  }, []);

  // One value for everything drawn inside the frame, the top bar's own button
  // included: the button that switches the width is a reader of it like any
  // screen is.
  const width = useMemo<LayoutWidth>(
    () => ({ fullWidth, toggleFullWidth }),
    [fullWidth, toggleFullWidth],
  );

  // The other frame-level choice, kept and read the same way. Memoized for
  // the same reason `toggleFullWidth` is: the palette registration and the
  // context value both key off identity.
  const toggleLargeText = useCallback(() => {
    setLargeText((was) => {
      const next = !was;
      try {
        localStorage.setItem(LAYOUT_TEXT_KEY, next ? "large" : "regular");
      } catch {
        // A browser that refuses storage still gets the session's choice.
      }
      return next;
    });
  }, []);

  const text = useMemo<LayoutText>(
    () => ({ largeText, toggleLargeText }),
    [largeText, toggleLargeText],
  );

  // The third frame-level choice, about what is listed rather than how the
  // frame is drawn: which way a domain's engrams are ordered. Kept and read
  // the way the width is, and memoized for the same reason.
  const setEngramsOrder = useCallback((next: EngramsOrder) => {
    setEngramsOrderState(next);
    try {
      localStorage.setItem(ENGRAMS_ORDER_KEY, next);
    } catch {
      // A browser that refuses storage still gets the session's choice.
    }
  }, []);
  const engramsOrderChoice = useMemo<EngramsOrderChoice>(
    () => ({ order: engramsOrder, setOrder: setEngramsOrder }),
    [engramsOrder, setEngramsOrder],
  );

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
          setHelpReturn(null);
          setHelpOpen(true);
        },
      },
      {
        id: "help.handbook",
        title: "Open the Handbook",
        run: () => {
          openWindow(HANDBOOK_URL, "noopener,noreferrer");
        },
      },
      // Named for the act rather than for the state it is in: a palette row
      // that read "Use full width" on one screen and "Use reading width" on
      // the next is one row a reader has to look twice at.
      {
        id: "layout.full-width",
        title: "Toggle full width",
        run: toggleFullWidth,
      },
      {
        id: "layout.large-text",
        title: "Toggle large text",
        run: toggleLargeText,
      },
    ];
    if (capabilities.canAdminister) {
      frame.push({
        id: "create-domain",
        title: "Create domain",
        run: () => {
          setCreatingDomain(true);
        },
      });
      frame.push({
        id: "github-settings",
        title: "GitHub settings",
        run: () => {
          void navigate(githubSettingsRoute());
        },
      });
    }
    // Registered only while it would act. The palette has no disabled row and
    // should not grow one: a row that opens a dialog to say there was nothing
    // to do is a door that does not open, and the button in the top bar is
    // where the reason belongs.
    if (share.visible && share.enabled) {
      frame.push({
        id: "share-changes",
        title: "Share changes",
        run: () => {
          openShare(share.domain);
        },
      });
    }
    return frame;
  }, [
    capabilities.canAdminister,
    navigate,
    openShare,
    share.domain,
    share.enabled,
    share.visible,
    toggleFullWidth,
    toggleLargeText,
  ]);
  useRegisterCommands(commands, "frame");

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "?" && event.key !== "\\") {
        return;
      }
      // Not while somebody is writing one. A bare key is only a shortcut
      // where no field has the focus, which includes the editor's own
      // contenteditable surface as much as it does a search box, and a
      // dropdown that jumps to the option a typed letter names. One guard
      // for both keys, because it is the same rule about the same thing.
      const target = event.target;
      if (
        target instanceof HTMLElement &&
        target.closest("input, textarea, select, [contenteditable=true]") !==
          null
      ) {
        return;
      }
      if (event.key === "?") {
        setHelpReturn(null);
        setHelpOpen(true);
        return;
      }
      // Cmd+\\ and Ctrl+\\ belong to the browser or to the system rather than
      // to this app, so those are refused. Alt is NOT a modifier to refuse
      // on: on a German, French, Spanish, Italian or Nordic layout the
      // backslash IS an Alt key - AltGr+ß or AltGr+8 on Windows and Linux,
      // which a browser reports as Ctrl and Alt together, and Shift+Option+7
      // on macOS. Refusing Alt would advertise a shortcut in the help
      // overlay that never fires on half the keyboards in Europe.
      if (event.metaKey || (event.ctrlKey && !event.altKey)) {
        return;
      }
      toggleFullWidth();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [toggleFullWidth]);

  return (
    // This tab's part in the browser's one change stream, taken here because
    // this is the authenticated shell: it mounts once, after login, and
    // never on the login or setup screens.
    <ChangeStreamProvider>
      <SidebarStateContext value={sidebarState}>
        <OutlineSlotContext value={outlineSlot}>
          {/*
        The width context goes around the whole frame rather than around the
        outlet: the top bar's own width button is a reader of this value too.
      */}
          <LayoutWidthContext value={width}>
            <LayoutTextContext value={text}>
              <EngramsOrderContext value={engramsOrderChoice}>
                <div className="min-h-screen bg-canvas text-ink">
                  <TopBar
                    navToggleRef={navToggleRef}
                    navOpen={navOpen}
                    onToggleNav={() => {
                      setNavOpen((open) => !open);
                    }}
                    onShare={openShare}
                    onHelp={(opener) => {
                      setHelpReturn(opener);
                      setHelpOpen(true);
                    }}
                    onCreateDomain={
                      capabilities.canAdminister
                        ? () => {
                            setCreatingDomain(true);
                          }
                        : null
                    }
                  />
                  <PhoneModeRow />
                  {/*
          Above everything, on every screen, for as long as this window is
          inside somebody else's draft. Working in another person's unfolded
          work is a state the WINDOW is in rather than something one page
          does, so it is drawn here and not by the screen that started it.
        */}
                  <JoinedDraftBar />
                  <div
                    className={`mx-auto flex w-full gap-6 px-4 md:gap-10 md:pl-0 ${
                      fullWidth ? "" : "max-w-350"
                    }`}
                  >
                    {/*
            The stored rail only reaches the sidebar where there is a sidebar
            to apply it to: below `md` this is a drawer, and it is always
            expanded.
          */}
                    <DomainSidebar
                      open={navOpen}
                      rail={rail && wide}
                      onToggleRail={toggleRail}
                      onDismiss={() => {
                        setNavOpen(false);
                      }}
                      drawerToggle={navToggleRef}
                      onCreateDomain={() => {
                        setCreatingDomain(true);
                      }}
                    />
                    {/*
            One attribute for the whole app's measure: the stylesheet lifts the
            cap under it, so no screen has to know anything about the choice to
            be drawn at the width it asks for. The frame's own cap goes with
            it: full width means the window, not a wider column inside the
            same box.
          */}
                    <main
                      ref={mainRef}
                      tabIndex={-1}
                      data-width={fullWidth ? "full" : undefined}
                      data-text={largeText ? "large" : undefined}
                      className="relative min-w-0 flex-1 py-6 focus:outline-none"
                    >
                      {capabilities.canAdminister && <GithubAccessBanner />}
                      <Outlet />
                      <ReadyFlash />
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
                    returnFocusTo={helpReturn}
                    onClose={() => {
                      setHelpOpen(false);
                    }}
                  />
                  {/*
          Mounted by the frame rather than by the sidebar, because both ways in
          - the launcher under the listing and the palette row - are the
          frame's.
        */}
                  {/*
          The sidebar's create dialogs, drawn here rather than inside the
          sidebar so a window crossing the breakpoint keeps them open, with
          what was typed in them.
        */}
                  {sidebarCreate?.kind === "engram" && (
                    <CreateEngramDialog
                      domain={sidebarCreate.domain}
                      initialFolder=""
                      returnFocusTo={sidebarCreate.launcher}
                      onClose={() => {
                        setSidebarCreate(null);
                      }}
                    />
                  )}
                  {sidebarCreate?.kind === "page" && (
                    <LazyBoundary fallback={null}>
                      <NewReflectionDialog
                        domain={sidebarCreate.domain}
                        folder={sidebarCreate.folder}
                        returnFocusTo={sidebarCreate.launcher}
                        onClose={() => {
                          setSidebarCreate(null);
                        }}
                      />
                    </LazyBoundary>
                  )}
                  {creatingDomain && (
                    <CreateDomainDialog
                      onClose={() => {
                        setCreatingDomain(false);
                      }}
                    />
                  )}
                  {/*
          The same pair, for the same reason: the button and the palette row
          both belong to the frame, and the picker hands straight over to the
          dialog beside it rather than opening a second one over itself.
        */}
                  {pickingShare && (
                    <SharePickerDialog
                      onPick={(domain) => {
                        setPickingShare(false);
                        setShareDomain(domain);
                      }}
                      onClose={() => {
                        setPickingShare(false);
                      }}
                    />
                  )}
                  {shareDomain !== null && (
                    <ShareDialog
                      domain={shareDomain}
                      onClose={() => {
                        setShareDomain(null);
                      }}
                    />
                  )}
                </div>
              </EngramsOrderContext>
            </LayoutTextContext>
          </LayoutWidthContext>
        </OutlineSlotContext>
      </SidebarStateContext>
    </ChangeStreamProvider>
  );
}

/**
 * Register item 18: a route change moves focus to the new screen's main
 * region. Without this, a keyboard or screen-reader user who followed a link
 * is left focused on an element of the previous screen - or on nothing.
 * The first render is exempt: the browser's own document focus is right.
 */
function RouteFocus({ target }: { target: RefObject<HTMLElement | null> }) {
  const location = useLocation();
  // A domain's bare address and its engram view's own address are one screen
  // when the bare address shows the engrams: the first filter or folder
  // written there moves the address to `/engrams` without a new screen
  // (0.28.1), and the reader's focus stays on the control they used.
  const pathname = location.pathname.replace(/^(\/d\/[^/]+)\/engrams$/, "$1");
  const first = useRef(true);
  // A screen reached by an action (a move, a delete) asks to focus its own
  // heading once it has drawn it (`useHeadingFocusRequest`); the frame stays
  // out of its way rather than racing it.
  const screenFocuses =
    (location.state as { focusHeading?: boolean } | null)?.focusHeading ===
    true;
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    if (!screenFocuses) {
      target.current?.focus();
    }
    // Only a change of route moves focus, not a change of its state.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pathname, target]);
  return null;
}

function TopBar({
  navToggleRef,
  navOpen,
  onToggleNav,
  onShare,
  onHelp,
  onCreateDomain,
}: {
  navToggleRef: RefObject<HTMLButtonElement | null>;
  navOpen: boolean;
  onToggleNav: () => void;
  /** Share the named domain, or open the picker when there is none. */
  onShare: (domain: string | null) => void;
  /** Open the keyboard shortcut map, the same one `?` opens. */
  onHelp: (opener: HTMLElement | null) => void;
  /** Register a domain, or null for a session that may not. */
  onCreateDomain: (() => void) | null;
}) {
  const { capabilities } = useAuth();
  const { fullWidth } = useFullWidth();
  const wide = useWide();
  const { domain, mode } = useDomainMode();

  return (
    <header className="sticky top-0 z-40 border-b border-hair bg-canvas/90 backdrop-blur print:hidden">
      {/*
        A fixed row rather than one sized by its contents: the sticky offsets
        below it - the reading surface's own sticky pieces, the editor's
        toolbar - are measured against this height, so it is a number the rest
        of the frame can rely on rather than whatever the tallest control in
        here happens to be today. The row follows the body's cap so the
        controls keep aligning with the content's edge at either width.
      */}
      <div
        className={`mx-auto flex h-14 w-full items-center gap-0.5 px-1 md:gap-2.5 md:pr-4 md:pl-5 ${
          fullWidth ? "" : "max-w-350"
        }`}
      >
        <IconButton
          ref={navToggleRef}
          label="Domains"
          icon={PanelLeft}
          onClick={onToggleNav}
          aria-expanded={navOpen}
          // The drawer exists only while open below the breakpoint, so the
          // reference is made only when there is something to point at.
          aria-controls={navOpen || wide ? "domain-sidebar" : undefined}
          className="md:hidden"
        />

        <Link
          to="/"
          className={`flex min-h-6 min-w-6 shrink-0 items-center gap-2 rounded text-[17px] font-semibold tracking-tight text-ink hover:opacity-80 lg:pr-1.5 ${FOCUS_RING}`}
        >
          <ShatterGem />
          <span className="sr-only lg:not-sr-only">Fluid</span>
        </Link>

        {/*
          The domain, then the mode inside it: what a reader is looking at,
          read from left to right. The slash is drawn, not spoken.
        */}
        <span
          aria-hidden="true"
          className="mx-1 h-5.5 w-px shrink-0 rotate-[18deg] bg-hair-strong"
        />
        <DomainSwitcher domain={domain} onCreateDomain={onCreateDomain} />
        {wide && domain !== "" && (
          <ReflectionModeSwitch domain={domain} current={mode} />
        )}

        <span className="grow" />

        {/*
          Both are drawn and the stylesheet picks one: below the large
          breakpoint the bar has no room left for a field, so search is an
          icon that opens the palette, handing itself over as the place the
          focus goes back to (Safari does not focus a clicked button).
        */}
        <SearchBox />
        <IconButton
          label="Open search"
          icon={Search}
          aria-haspopup="dialog"
          className="lg:hidden"
          onClick={(event) => {
            openPalette(event.currentTarget);
          }}
        />

        <ShareChanges onShare={onShare} />

        {capabilities.readOnly && (
          <span className="hidden md:inline">
            <Chip variant="caution">Read only</Chip>
          </span>
        )}

        <AvatarMenu onShortcuts={onHelp} />
      </div>
    </header>
  );
}

/**
 * The domain the address is inside ("" outside every domain) and which of
 * its two modes the screen belongs to.
 *
 * `useMatch` rather than `useParams`, for the reason `DomainSidebar` spells
 * out: inside a layout route the params are the layout's own, which are none.
 */
function useDomainMode(): {
  domain: string;
  mode: "engrams" | "reflection";
} {
  const match = useMatch("/d/:domain/*");
  return {
    domain: match?.params.domain ?? "",
    mode:
      reflectionPathOf(match?.params["*"] ?? "") === null
        ? "engrams"
        : "reflection",
  };
}

/**
 * The mode switch on a phone: a row of its own under the bar, at full width,
 * scrolling away with the page (spec 4.5). Above the medium breakpoint it
 * sits in the bar instead; one place at a time, never both.
 */
function PhoneModeRow() {
  const wide = useWide();
  const { domain, mode } = useDomainMode();
  const status = useReflectionStatus(domain);
  // The listing is the eager answer that is usually there first, so the row
  // keeps its height from the start instead of pushing the page down a beat
  // later; the domain's own status decides once it has answered.
  const listing = useQuery({
    queryKey: REFLECTION_DOMAINS_KEY,
    queryFn: fetchReflectionDomains,
  });
  const listed = listing.data?.find((row) => row.domain === domain);
  const facts = status.data ?? listed;
  if (wide || facts === undefined || switchHome(facts) === false) {
    return null;
  }
  return (
    <div className="flex h-12 items-center border-b border-hair px-3 print:hidden">
      <ReflectionModeSwitch
        domain={domain}
        current={mode}
        fill
        listed={listed}
      />
    </div>
  );
}

/**
 * Sharing what this instance has learned, from wherever the reader is.
 *
 * Beside the maintenance link and for the same reason it stands there: it is an
 * act about the whole instance rather than about the screen in front of you,
 * and the sidebar hands itself over to a domain's own navigation as soon as
 * somebody opens one. It is a button rather than a link because it has a face
 * for "not now", which no link has.
 *
 * That face is `aria-disabled` rather than `disabled`, and the difference is
 * the whole point of drawing it: a disabled button takes no focus and emits no
 * pointer events, so the tooltip saying WHY it will not act would be a label
 * nobody could ever read. This one is reachable by pointer and by keyboard,
 * says it will not act, and does not - the press is guarded as well as the
 * face, because a control a pointer can reach is a control a pointer can press.
 *
 * The name is drawn where there is room for it and spoken either way, the way
 * the link beside it does it: below the medium breakpoint the word folds away
 * and the glyph carries it, and the `aria-label` makes the two widths one
 * control to anything that reads the page.
 */
function ShareChanges({
  onShare,
}: {
  onShare: (domain: string | null) => void;
}) {
  const share = useShareAction();
  if (!share.visible) {
    return null;
  }

  return (
    // Always a tooltip, and its label is the reason whenever there is one.
    // Not a nicety: dropping the wrapper once the action came alive would
    // unmount and rebuild the button under a reader who had just focused it,
    // and below the medium breakpoint the tooltip is also the only place the
    // folded-away word is drawn for a pointer.
    <Tooltip label={share.reason ?? share.waiting ?? "Share changes"}>
      <button
        type="button"
        aria-label="Share changes"
        aria-disabled={!share.enabled}
        onClick={() => {
          if (share.enabled) {
            onShare(share.domain);
          }
        }}
        className={`inline-flex h-8 shrink-0 items-center gap-1.5 rounded px-2 text-sm text-slate-600 hover:bg-slate-100 aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:text-slate-300 dark:hover:bg-slate-800 dark:aria-disabled:hover:bg-transparent ${FOCUS_RING}`}
      >
        <Share2 aria-hidden="true" size={16} strokeWidth={1.75} />
        <span className="hidden md:inline">Share</span>
        {/*
          How much is waiting, drawn rather than spoken: the label stays the
          one control name a reader hears at either width, and the tooltip
          above says the same number in words - with the pairing, where the
          server sent one - so nothing here is only visible.
        */}
        {share.count !== null && (
          <span aria-hidden="true" className="text-caption tabular-nums">
            {share.count}
          </span>
        )}
      </button>
    </Tooltip>
  );
}

/**
 * The search box. It only routes; the search screen owns the query itself.
 *
 * It searches every domain, inside a domain too (the search screen narrows
 * with its own domain chips), so the placeholder says so everywhere rather
 * than naming the domain on screen.
 */
function SearchBox() {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");

  return (
    <form
      role="search"
      className="hidden w-full max-w-[340px] min-w-0 shrink lg:block"
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
        <Search
          aria-hidden="true"
          size={16}
          strokeWidth={1.75}
          className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-faint"
        />
        <input
          id="topbar-search"
          type="search"
          name="q"
          value={query}
          placeholder="Search everything"
          onChange={(event) => {
            setQuery(event.target.value);
          }}
          className={`h-[34px] w-full rounded-lg border border-transparent bg-sunken pr-14 pl-8.5 text-[13.5px] text-ink outline-none placeholder:text-faint ${FOCUS_RING}`}
        />
        <kbd
          aria-hidden="true"
          className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 rounded border border-hair-strong bg-raised px-1.5 text-[11px] text-muted"
        >
          {PALETTE_HINT}
        </kbd>
      </div>
    </form>
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
 * sidebar takes the rest of the window's height. A long tree therefore scrolls
 * inside the sidebar while the reading surface scrolls the page, which is what
 * keeps the way around a domain reachable from the bottom of a long engram,
 * and the fold control in its foot stays in one place.
 */
function DomainSidebar({
  open,
  rail,
  onToggleRail,
  onDismiss,
  drawerToggle,
  onCreateDomain,
}: {
  open: boolean;
  rail: boolean;
  onToggleRail: () => void;
  /** Shut the phone drawer from inside it (close button, Escape, scrim). */
  onDismiss: () => void;
  /** The bar's drawer toggle, where the keyboard goes when the drawer shuts. */
  drawerToggle: RefObject<HTMLButtonElement | null>;
  /** Open the frame's own register-a-domain dialog. */
  onCreateDomain: () => void;
}) {
  const navigate = useNavigate();
  const { capabilities } = useAuth();
  const wide = useWide();
  const sidebar = useSidebarState();
  const outline = useContext(OutlineSlotContext)?.node ?? null;
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });

  // The fold is one button in the foot whose name and glyph flip, so the
  // keyboard stays where it was: after a press the button is focused again,
  // in case the fold redrew it, rather than the reader landing on the
  // document. Only after a press: a mount that read the stored preference
  // must not steal focus from wherever the reader is.
  const toggleRef = useRef<HTMLButtonElement | null>(null);
  const pressed = useRef(false);
  // Whether the drawer was shut by the reader rather than by a link.
  const returnToToggle = useRef(false);
  const drawerRef = useRef<HTMLDivElement | null>(null);
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
  const { canWrite } = useDomainWritable(domain);
  // The splat holds whatever follows the domain: `e/<permalink>` on an engram
  // screen, `edit/<permalink>` in the editor, the empty string on the
  // domain's own home. Both prefixed forms carry the same permalink, so the
  // sidebar highlights the right tree row and offers its own launcher on
  // either screen, and only the domain's own home - where the permalink is
  // truly absent - is treated as having none.
  const rest = match?.params["*"] ?? "";
  // On a Reflection route the sidebar is the Reflection's own tree: the
  // engram tree is the wrong map there, and for a contributor it is a 404.
  const reflectionPath = reflectionPathOf(rest);
  const permalink = rest.startsWith("e/")
    ? rest.slice(2)
    : rest.startsWith("edit/")
      ? rest.slice(5)
      : "";
  const content = rail ? (
    /*
      Folded: the two things somebody in the middle of reading still needs
      from the frame. Names rather than bare glyphs, because an icon column is
      exactly where a keyboard or a screen reader has the least to go on. The
      way back out of the fold is in the foot, where the fold itself was.
    */
    <div className="flex flex-col items-center gap-1">
      <IconButton
        label="All domains"
        icon={House}
        onClick={() => {
          void navigate("/");
        }}
      />
      {/*
        The same condition `DomainNav` puts on its own launcher, for the same
        reason: the domain's home screen carries one beside its heading,
        prefilled with the folder being browsed, and the frame offering a
        second one would be two controls for one act.
      */}
      {permalink !== "" && canWrite && (
        <IconButton
          label="New engram"
          icon={Plus}
          onClick={(event) => {
            sidebar?.create({
              kind: "engram",
              domain,
              launcher: event.currentTarget,
            });
          }}
        />
      )}
    </div>
  ) : domain === "" ? (
    <DomainList
      domains={listing.data?.domains}
      pending={listing.isPending}
      error={listing.error}
      // Only for the session that may register one. The server refuses
      // everyone else anyway, so this is the difference between a sidebar
      // that offers what you can do and one that offers a door that will not
      // open.
      onCreateDomain={capabilities.canAdminister ? onCreateDomain : null}
    />
  ) : reflectionPath !== null ? (
    <LazyBoundary fallback={null}>
      <ReflectionNav domain={domain} path={reflectionPath} />
    </LazyBoundary>
  ) : (
    <>
      {/*
        A listing that failed is said here as it is in the flat mode. The
        tree is drawn either way: it comes from the domain's own address, so
        it works even when nothing could be listed.
      */}
      {listing.error && <SidebarProblem error={listing.error} />}
      <DomainNav domain={domain} permalink={permalink} />
    </>
  );
  const label = domain === "" ? "Domains" : `Domain ${domain}`;

  // Below the medium breakpoint the sidebar is a drawer: a modal dialog from
  // the left edge, on the same radix primitive every other dialog here uses,
  // so the focus trap, Escape, the scroll lock and hiding the page from a
  // screen reader come with it. Its head names the domain and carries the
  // way shut.
  if (!wide) {
    return (
      <Dialog.Root
        open={open}
        onOpenChange={(next) => {
          if (!next) {
            returnToToggle.current = true;
            onDismiss();
          }
        }}
      >
        <Dialog.Portal>
          <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 print:hidden" />
          <Dialog.Content
            ref={drawerRef}
            aria-describedby={undefined}
            aria-modal="true"
            // The drawer itself takes the keyboard rather than its close
            // button, so no tooltip pops up on the way in.
            onOpenAutoFocus={(event) => {
              event.preventDefault();
              (event.currentTarget as HTMLElement).focus();
            }}
            // Escape belongs to the dialog on top. Radix's layer stack already
            // sees to that; this is belt-and-braces for a dialog opened from
            // the drawer in the same tick as the key, before its layer has
            // registered, so the drawer never shuts under an open dialog.
            onEscapeKeyDown={(event) => {
              const dialogs = document.querySelectorAll(
                "[role=dialog], [role=alertdialog]",
              );
              if (
                Array.from(dialogs).some(
                  (dialog) => dialog !== drawerRef.current,
                )
              ) {
                event.preventDefault();
              }
            }}
            // Back to the bar's toggle when the reader shut it; a link
            // followed out of it leaves the keyboard to the new screen.
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              if (returnToToggle.current) {
                drawerToggle.current?.focus();
              }
              returnToToggle.current = false;
            }}
            className="fixed inset-y-0 left-0 z-50 flex w-[300px] max-w-[85vw] flex-col bg-chrome shadow-pop outline-none print:hidden"
          >
            <div className="flex items-center gap-2 px-3 pt-3 pb-1">
              <Dialog.Title className="min-w-0 grow truncate px-2.5 text-base font-semibold text-ink">
                {domain === "" ? "All domains" : domain}
              </Dialog.Title>
              <IconButton
                label="Close the sidebar"
                icon={X}
                onClick={() => {
                  returnToToggle.current = true;
                  onDismiss();
                }}
              />
            </div>
            <nav
              id="domain-sidebar"
              aria-label={label}
              className="min-h-0 flex-1 overflow-y-auto px-3 py-4"
            >
              {/* A long page's outline leads the drawer (spec B1). */}
              <div className="mb-4 border-b border-hair pb-4 empty:hidden">
                {outline}
              </div>
              {content}
            </nav>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    );
  }

  // From the breakpoint up it is a column under the top bar that sticks
  // while the page scrolls, inside a column that carries the chrome colour
  // and the hairline on its right down the whole page; the shadow carries that colour on to the window's left edge when
  // the frame is capped narrower than the window.
  return (
    <div
      className={`shrink-0 border-r border-hair bg-chrome print:hidden ${
        rail
          ? "w-12 shadow-[-48px_0_0_var(--chrome),-96px_0_0_var(--chrome),-144px_0_0_var(--chrome),-192px_0_0_var(--chrome)]"
          : "w-64 shadow-[-256px_0_0_var(--chrome)]"
      }`}
    >
      <nav
        id="domain-sidebar"
        aria-label={label}
        className="sticky top-14 flex h-[calc(100dvh-3.5rem)] flex-col"
      >
        {/*
        The rows scroll on their own, under a foot that stays put. The side
        padding is wide enough for the focus ring: a box that scrolls on one
        axis clips the other one too.
      */}
        <div
          className={`min-h-0 flex-1 overflow-y-auto py-4 ${rail ? "px-2" : "px-3"}`}
        >
          {content}
        </div>
        {/* The fold, in the foot at the bottom right (spec 4.4). */}
        <div
          className={`flex shrink-0 py-2 ${rail ? "justify-center" : "justify-end px-3"}`}
        >
          <IconButton
            ref={toggleRef}
            label={rail ? "Expand the sidebar" : "Collapse the sidebar"}
            icon={rail ? PanelLeftOpen : PanelLeftClose}
            aria-expanded={!rail}
            aria-controls="domain-sidebar"
            onClick={fold}
          />
        </div>
      </nav>
    </div>
  );
}

/** Every domain this instance holds: the sidebar outside any one of them. */
function DomainList({
  domains,
  pending,
  error,
  onCreateDomain,
}: {
  domains: DomainSummary[] | undefined;
  pending: boolean;
  error: Error | null;
  /** Register a domain, or null for a session that may not. */
  onCreateDomain: (() => void) | null;
}) {
  // Which screen this listing is drawn beside, for the launcher below: the
  // home screen is the one that carries its own.
  const home = useLocation().pathname === "/";
  // A contributor's domains are not in `domains` (it lists only those whose
  // engrams the caller reads), so "empty" needs the Reflection listing too.
  // Same key as Home: one request.
  const reflections = useQuery({
    queryKey: REFLECTION_DOMAINS_KEY,
    queryFn: fetchReflectionDomains,
  });
  // A failed Reflection listing counts as none: the sentence is about what
  // this list can show, and the failure is not worth hiding it for.
  const noReflections = reflections.data?.length === 0 || reflections.isError;
  return (
    <>
      {/* The first label of the column, so it does not need the gap above it. */}
      <h2 className={SIDE.label.replace("pt-[18px]", "pt-0")}>Domains</h2>
      {pending && (
        <Loading compact label="Loading domains" className="px-2.5 py-1.5" />
      )}
      {error && <SidebarProblem error={error} />}
      <ul className="flex flex-col gap-0.5">
        {domains?.map((domain) => (
          <li key={domain.name}>
            <DomainLink domain={domain} />
          </li>
        ))}
      </ul>
      {domains?.length === 0 && noReflections && (
        <p className="px-2.5 py-1.5 text-sm text-muted">
          No domains are registered on this instance yet.
        </p>
      )}
      {/*
        Under the listing, in the launcher shape `DomainNav` gives "New
        engram": the same place in the same column, doing the same kind of
        thing one level up. It is also what the empty state above is an
        invitation to - an instance with nothing on it says so and then offers
        the one act that changes that, rather than saying so and stopping.

        Left out on the home screen only, which is `DomainNav`'s own rule one
        level down: that screen carries the same launcher beside its Domains
        heading, so the sidebar would otherwise offer a second one right next
        to the first. Every other screen outside a domain - users, settings,
        search, the graph - draws this listing with no launcher of its own, and
        keeps this one.
      */}
      {onCreateDomain !== null && home === false && (
        <button
          type="button"
          onClick={onCreateDomain}
          className={`${SIDE.row} ${SIDE.add} mt-1.5 w-full`}
        >
          <Plus
            aria-hidden="true"
            size={16}
            strokeWidth={1.75}
            className={SIDE.icon}
          />
          New domain
        </button>
      )}
    </>
  );
}

function DomainLink({ domain }: { domain: DomainSummary }) {
  return (
    <NavLink
      to={domainRoute(domain.name)}
      {...PREFETCH.domain}
      className={({ isActive }) =>
        `${SIDE.row} justify-between ${isActive ? SIDE.current : ""}`
      }
    >
      {/*
        The name and its badge travel together on the left, so the count stays
        pinned to the right edge whether or not a badge is there. Inside the
        link rather than beside it, unlike the home card and the domain
        screen: the whole row is the target here, and a chip hanging outside
        it would be the one part of the row that does not take a click. The
        link then reads "lab private 12", which is the row said out loud.
      */}
      <span className="flex min-w-0 items-center gap-1.5">
        <span className="truncate">{domain.name}</span>
        {domain.private && <Chip variant="accent">private</Chip>}
      </span>
      {domain.engrams !== null && (
        <span className={SIDE.count}>{domain.engrams}</span>
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
