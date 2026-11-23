/**
 * Every address the app answers at.
 *
 * Declarative routing rather than a data router: the data layer is TanStack
 * Query, so the router has nothing to load and only has to match URLs. It is
 * also what lets the whole tree mount inside a `MemoryRouter` in a test with
 * no second route definition to keep in step. The builders that make links to
 * these patterns live in `paths.ts`, next door.
 *
 * `/evolve` is deliberately absent. That is the endpoint's name and the name of
 * the tool an agent works the queue with; the screen a person opens to see what
 * is owed lives at `/maintenance`, and the bare `/evolve` lands on the
 * not-found screen rather than existing as a second address for one page.
 */

import { useQueryClient } from "@tanstack/react-query";
import { Suspense, lazy, useEffect, useState } from "react";
import {
  Navigate,
  Route,
  Routes,
  useLocation,
  useParams,
  useSearchParams,
} from "react-router";

import { useAuth } from "./auth/AuthContext";
import LoginPage from "./auth/LoginPage";
import { RequireAuth } from "./auth/RequireAuth";
import { Layout } from "./components/Layout";
import { LazyBoundary } from "./components/LazyBoundary";
import { Loading } from "./components/Loading";
import { lazyChunk } from "./lazyChunk";
import {
  domainStayKey,
  opensOnReflection,
  useReflectionStatus,
} from "./components/ReflectionModeSwitch";
import {
  adminGeneralRoute,
  engramsRoute,
  githubSettingsRoute,
  reflectionRoute,
  usersRoute,
} from "./paths";
import Home from "./screens/Home";
import NotFound from "./screens/NotFound";
import OauthConsent from "./screens/OauthConsent";

/**
 * Three screens a reader opens from the frame rather than lands on: the
 * search results, the full-screen graph and the maintenance queue. They were
 * eager once, and every visitor paid for them before the first screen ran,
 * whether or not they ever opened one. Lazy for the reason the reading screen
 * is: the daemon serves this bundle off localhost, so the chunk is a round
 * trip of a few milliseconds. Maintenance waits behind the skeleton it
 * draws itself while its sweep runs, so the chunk and the sweep read as one
 * wait; search and the graph have no loading state of their own, so each
 * waits behind a plain labelled skeleton.
 */
const Search = lazyChunk(() => import("./screens/Search"));

/**
 * A domain's front page and its folders, and the domain's settings. Lazy like
 * the reading screen: the front page draws a MANIFEST through the markdown
 * renderer, a rail and the engram list, and the settings carry every team
 * card, none of which the app shell should pay for on every other screen.
 */
const DomainHome = lazyChunk(() => import("./screens/DomainHome"));
const DomainSettings = lazyChunk(() => import("./screens/DomainSettings"));
const GraphView = lazyChunk(() => import("./screens/GraphView"));
const Maintenance = lazyChunk(() => import("./screens/Maintenance"));

/**
 * The two editors: everything CodeMirror rides in these chunks, so the
 * app shell never pays for either editor. Split from one another rather than
 * shared: a MANIFEST editor has no wikilinks, no frontmatter form and no
 * vocabulary to fetch, and bundling it with the engram editor's weight would
 * tax a MANIFEST edit for panels it never draws. The fallback matches the
 * engram screen's own skeleton so a prefetch miss is a quiet moment rather
 * than a flash; the MANIFEST editor shares it rather than inventing a second
 * one.
 */
const EngramEditor = lazyChunk(() => import("./screens/EngramEditor"));
const ManifestEditor = lazyChunk(() => import("./screens/ManifestEditor"));

/**
 * The reading screen, lazy for a third reason again.
 *
 * It is the screen this app exists to draw, so making it wait looks wrong
 * until the numbers are read: it pulls the markdown renderer's whole
 * neighborhood, the details rail (Linked from and Explore), the two dialogs and the
 * neighborhood panel behind it, and every one of those bytes was being paid
 * for by the home screen, the search screen and the login screen too. The
 * primary deployment is the daemon serving this bundle off localhost, where a
 * second request for an already-built chunk is a round trip measured in
 * single-digit milliseconds - and the fallback is the exact skeleton the
 * screen shows itself while its own two requests are in flight, so a reader
 * sees one loading shape rather than a chunk fetch followed by a screen.
 */
const EngramPage = lazyChunk(() => import("./screens/EngramPage"));

/**
 * And the Administration area, lazy for a different reason: its pages weigh
 * little each, but only an admin ever opens them, and the app shell is paid
 * for by everybody who opens the app at all. The frame and its small pages
 * (General) share one chunk; the accounts table, the agents, the invites and
 * the GitHub connection keep a chunk each, the accounts table being the
 * largest and taking the most off the eager path.
 */
const AdminArea = lazyChunk(() => import("./screens/AdminArea"));
const AdminGeneral = lazyChunk(() =>
  import("./screens/AdminArea").then((m) => ({ default: m.AdminGeneral })),
);
const AdminAgents = lazyChunk(() => import("./screens/AdminAgents"));
const AdminInvites = lazyChunk(() => import("./screens/AdminInvites"));
const GithubSettings = lazyChunk(() => import("./screens/GithubSettings"));
const UsersAdmin = lazyChunk(() => import("./screens/UsersAdmin"));

/**
 * And the profile, lazy for the third of those reasons: it is a screen a reader
 * opens once, to connect the identity they share as, and never again.
 */
const Profile = lazyChunk(() => import("./screens/Profile"));

/**
 * Where a draft share-link lands. Lazy for the reason the two admin screens
 * are: almost nobody ever opens it, and the app shell is paid for by
 * everybody. It carries no editor and no graph engine of its own - a granted
 * draft is one page handed over by its author - so the chunk is small and the
 * wait is one request on a screen that is itself the arrival.
 */
const GrantedDraft = lazyChunk(() => import("./screens/GrantedDraft"));

/**
 * The Reflection's screens, lazy for the reason the engram screens are: a
 * domain's pages for people carry the markdown renderer, the co-editing
 * client and the review lists, and the app shell is paid for by everybody.
 */
const ReflectionHome = lazyChunk(() => import("./screens/ReflectionHome"));
const ReflectionPage = lazyChunk(() => import("./screens/ReflectionPage"));
const ReflectionReview = lazyChunk(() => import("./screens/ReflectionReview"));
const ReflectionEditor = lazyChunk(() => import("./screens/ReflectionEditor"));

/**
 * The station itself, every place in it under one splat, `/π/*`, lazy like
 * the screens above so the app shell never pays for it.
 *
 * It ships in every build: the C64 screen behind the header's gem is its
 * way in (`LOAD"GAME",8,1`). It sits inside `RequireAuth`, since every room
 * is built from what the account reads, and outside `Layout`: the station
 * is full screen, with no frame around it. The route reads the location
 * itself (`addressOfGameLocation` in `game/paths.ts`): `/π` is the airlock,
 * `/π/d/<domain>` a domain's bridge, the same with `?path=<folder>` a deck
 * and `/π/d/<domain>/e/<permalink>` an engram's room, each Fluid's own
 * address under the `π` prefix, and any other path under it the airlock.
 * That prefix is the Unicode character itself, deliberately hard to type:
 * the station is an Easter egg, meant to be reached from the C64 screen
 * rather than typed in, and there is no `/game` alias into it.
 */
const ExploreRoute = lazy(() => import("./game/ExploreRoute"));

/**
 * The station's look demo at `/π/dev`, and only in development.
 *
 * `import.meta.env.DEV` is a constant the build replaces, so in a production
 * build this is `null`, the import below is dead code, and neither the
 * chunk nor the route exists. Its route sits beside the station's, inside
 * `RequireAuth` and outside `Layout`, and outranks the station's splat by
 * being more specific.
 */
const LookDemo = import.meta.env.DEV
  ? lazy(() => import("./game/dev/LookDemo"))
  : null;

/**
 * The model gallery at `/π/dev/gallery`: one room with every model the
 * station draws, for judging them. A development tool, like the look demo,
 * and gated the same way for good: the station ships, its dev seams never
 * do.
 */
const Gallery = import.meta.env.DEV
  ? lazy(() => import("./game/dev/Gallery"))
  : null;

/**
 * The sound board at `/π/dev/sounds`: every patch the station plays, one
 * button each (M4 C28). A development tool, gated the same way for good.
 */
const SoundBoard = import.meta.env.DEV
  ? lazy(() => import("./game/dev/SoundBoard"))
  : null;

/**
 * The MANIFEST's old address. The document is read on the engram view now,
 * so a link that still points here lands there rather than on the not-found
 * screen; the editor keeps its own segment beneath it.
 */
function ManifestRedirect() {
  const { domain = "" } = useParams();
  return <Navigate to={engramsRoute(domain)} replace />;
}

/**
 * An address an admin page had before the Administration area (0.35.0):
 * `/users` and `/settings/github`. For an admin it goes on to the page's new
 * address with its query and fragment, so a bookmark or the link back from
 * GitHub (`?app=created`) still lands with what it carried; the router's
 * basename keeps the base path. Anyone else gets the not-found screen in
 * place, the answer both addresses always gave them, so the old address says
 * no more than a mistyped one does.
 */
function MovedAdminPage({ to }: { to: string }) {
  const { capabilities } = useAuth();
  const location = useLocation();
  if (!capabilities.canAdminister) {
    return <NotFound />;
  }
  return <Navigate to={`${to}${location.search}${location.hash}`} replace />;
}

/**
 * The domain's own address, `/d/<domain>`, and the one place that decides
 * which view a domain opens on (0.28.1).
 *
 * A query means the engram view: folders (`?path=`) and frontmatter filters
 * lived on this address before the engram view had one of its own, so a link
 * somebody saved goes on to `/d/<domain>/engrams` with its query, fragment
 * and state, before the status is asked. Otherwise the Reflection status
 * decides: in effect here (`on`) and readable, the domain opens on its
 * Reflection, with the fragment kept. A contributor, who cannot read
 * engrams, goes there too, since it is their only view. Every other answer (off, pending, elsewhere, hosted
 * elsewhere, homeless, no right, a failed read) shows the engram view in
 * place, which answers a missing domain or a pending Reflection as it always
 * did. While the status loads, the domain's loader stands, so the engram
 * view never flashes before a forward.
 *
 * The engram view's own address renders this same component with
 * `engramsView`, which skips the decision. Both addresses then draw the same
 * tree (this component, the boundary, DomainHome), so a hop from one to the
 * other, such as the first filter written on the bare address, keeps the
 * page mounted with its focus and state.
 */
function DomainEntry({ engramsView = false }: { engramsView?: boolean }) {
  const { domain = "" } = useParams();
  const location = useLocation();
  const status = useReflectionStatus(domain);
  // The domain this address already decided to show on its engrams. Once
  // decided it holds: a later read of the status (the engram view mounts its
  // own observer, which asks again after a failure, and the status goes back
  // to pending while it does) must not take the page away and put it back.
  const [stayed, setStayed] = useState<string | null>(null);
  // The decision, told to the mode switch in the frame, which marks the half
  // this page shows; cleared when the page goes, so the next visit to the
  // bare address marks nothing until it has decided again.
  const queryClient = useQueryClient();
  const staying = stayed === domain;
  useEffect(() => {
    if (!staying) {
      return;
    }
    queryClient.setQueryData(domainStayKey(domain), true);
    return () => {
      queryClient.removeQueries({ queryKey: domainStayKey(domain) });
    };
  }, [domain, queryClient, staying]);
  if (!engramsView && location.search !== "") {
    return (
      <Navigate
        to={`${engramsRoute(domain)}${location.search}${location.hash}`}
        state={location.state as unknown}
        replace
      />
    );
  }
  if (!engramsView && stayed !== domain) {
    if (status.isPending) {
      return DOMAIN_FALLBACK;
    }
    if (status.data !== undefined && opensOnReflection(status.data)) {
      return (
        <Navigate to={`${reflectionRoute(domain)}${location.hash}`} replace />
      );
    }
    setStayed(domain);
  }
  return (
    <LazyBoundary fallback={DOMAIN_FALLBACK}>
      <DomainHome />
    </LazyBoundary>
  );
}

/*
 * Every Fluid route below that loads on demand waits behind `LazyBoundary`
 * with one of these fallbacks: the loading shape while its chunk is on the
 * way, and a reload prompt if the chunk never arrives (a tab left open across
 * a deploy). The station's routes keep their plain `Suspense`.
 */
const EDITOR_FALLBACK = (
  <Loading label="Loading the editor" subject="the editor" />
);

const SETTINGS_FALLBACK = (
  <Loading label="Loading settings" subject="the settings" />
);

const USERS_FALLBACK = (
  <Loading label="Loading accounts" subject="the accounts" />
);

const ADMIN_FALLBACK = (
  <Loading label="Loading administration" subject="the administration" />
);

const PROFILE_FALLBACK = (
  <Loading label="Loading your profile" subject="your profile" />
);

/**
 * The engram screen's own loader, spelled the same way it spells it: the
 * chunk arriving and the detail request landing are one wait to a reader, and
 * two different loading shapes in a row would say otherwise. The first line
 * names the address, which is all the route knows before the data lands.
 */
function EngramFallback() {
  const { "*": permalink = "" } = useParams();
  return <Loading label="Loading the engram" subject={permalink} />;
}
const ENGRAM_FALLBACK = <EngramFallback />;

function ReflectionFallback() {
  const { domain = "", "*": path = "" } = useParams();
  return (
    <Loading
      label="Loading the Reflection"
      subject={path === "" ? `${domain} reflection` : path}
    />
  );
}
const REFLECTION_FALLBACK = <ReflectionFallback />;

function DomainFallback() {
  const { domain = "" } = useParams();
  return <Loading label="Loading the domain" subject={domain} />;
}
const DOMAIN_FALLBACK = <DomainFallback />;

function SearchFallback() {
  const [params] = useSearchParams();
  return (
    <Loading
      label="Loading search"
      subject={params.get("q") ?? "your search"}
    />
  );
}
const SEARCH_FALLBACK = <SearchFallback />;

const GRAPH_FALLBACK = (
  <Loading label="Loading the graph" subject="the graph" />
);

/**
 * The loader the maintenance screen draws itself while its sweep is in
 * flight, so the chunk and the sweep read as one wait.
 */
const MAINTENANCE_FALLBACK = (
  <Loading label="Sweeping the domains" subject="the domains" />
);

const GRANTED_DRAFT_FALLBACK = (
  <Loading label="Opening the shared draft" subject="the shared draft" />
);

export function AppRoutes() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route element={<RequireAuth />}>
        {/*
          The look demo, full screen and so outside `Layout`. In a
          production build `LookDemo` is null and the route does not exist.
          The dev routes are declared above the station's splat, which they
          outrank anyway by being more specific.
        */}
        {LookDemo === null ? null : (
          <Route
            path="/π/dev"
            element={
              <Suspense fallback={null}>
                <LookDemo />
              </Suspense>
            }
          />
        )}
        {/*
          The gallery, gated and placed the same way.
        */}
        {Gallery === null ? null : (
          <Route
            path="/π/dev/gallery"
            element={
              <Suspense fallback={null}>
                <Gallery />
              </Suspense>
            }
          />
        )}
        {/*
          The sound board, gated and placed the same way.
        */}
        {SoundBoard === null ? null : (
          <Route
            path="/π/dev/sounds"
            element={
              <Suspense fallback={null}>
                <SoundBoard />
              </Suspense>
            }
          />
        )}
        {/*
          The station itself, in every build: full screen, outside `Layout`.
        */}
        <Route
          path="/π/*"
          element={
            <Suspense fallback={null}>
              <ExploreRoute />
            </Suspense>
          }
        />
        <Route element={<Layout />}>
          <Route index element={<Home />} />
          <Route path="/d/:domain" element={<DomainEntry />} />
          {/*
            The engram view: the domain's MANIFEST, folders and engrams. The
            bare domain address above opens on the Reflection when it is on,
            so a link that means the engrams comes here. The same element
            type as the route above, so a hop between the two keeps the page.
          */}
          <Route
            path="/d/:domain/engrams"
            element={<DomainEntry engramsView />}
          />
          <Route
            path="/d/:domain/settings"
            element={
              <LazyBoundary fallback={SETTINGS_FALLBACK}>
                <DomainSettings />
              </LazyBoundary>
            }
          />
          {/*
            The permalink is a path of its own, so it arrives through the
            splat rather than a named param: `/d/eng/e/notes/deep/gamma` is
            one engram, not a missing route.
          */}
          {/*
            The editor is not under `/e/`: that pattern ends in a splat, and a
            splat swallows every segment after it, so `edit` gets its own
            segment ahead of the permalink.
          */}
          <Route
            path="/d/:domain/edit/*"
            element={
              <LazyBoundary fallback={EDITOR_FALLBACK}>
                <EngramEditor />
              </LazyBoundary>
            }
          />
          {/*
            Both MANIFEST routes sit above `/d/:domain/e/*` for readability,
            though nothing here collides: `manifest` is its own segment, not a
            permalink the splat below would otherwise swallow. The first is a
            redirect, kept so a link somebody saved still lands where the
            MANIFEST is read now, the domain page; the editor rides its own
            lazy chunk, gated to admins by the screen itself the same way
            the Administration area is.
          */}
          <Route path="/d/:domain/manifest" element={<ManifestRedirect />} />
          <Route
            path="/d/:domain/manifest/edit"
            element={
              <LazyBoundary fallback={EDITOR_FALLBACK}>
                <ManifestEditor />
              </LazyBoundary>
            }
          />
          <Route
            path="/d/:domain/reflection"
            element={
              <LazyBoundary fallback={REFLECTION_FALLBACK}>
                <ReflectionHome />
              </LazyBoundary>
            }
          />
          <Route
            path="/d/:domain/reflection/review"
            element={
              <LazyBoundary fallback={REFLECTION_FALLBACK}>
                <ReflectionReview />
              </LazyBoundary>
            }
          />
          {/*
            One page of a domain's Reflection. The splat is the page's path
            inside `reflections/`; it always ends in `.md`, so the static
            `review` route (Task 10) can never be a page's address.
          */}
          <Route
            path="/d/:domain/reflection/*"
            element={
              <LazyBoundary fallback={REFLECTION_FALLBACK}>
                <ReflectionPage />
              </LazyBoundary>
            }
          />
          {/*
            The live editor over one page: `reflection-edit` is its own
            segment ahead of the splat, the way `edit` is for engrams.
          */}
          <Route
            path="/d/:domain/reflection-edit/*"
            element={
              <LazyBoundary fallback={EDITOR_FALLBACK}>
                <ReflectionEditor />
              </LazyBoundary>
            }
          />
          <Route
            path="/d/:domain/e/*"
            element={
              <LazyBoundary fallback={ENGRAM_FALLBACK}>
                <EngramPage />
              </LazyBoundary>
            }
          />
          {/*
            A link somebody was handed, which is the one address a person
            arrives at from outside the app entirely. The token is opaque and
            rides as one segment; presenting it is what binds the grant to the
            account signed in here, so the route sits inside `RequireAuth`
            like every other: a link binds to an account, and the anonymous
            viewer has none.
          */}
          <Route
            path="/draft/:token"
            element={
              <LazyBoundary fallback={GRANTED_DRAFT_FALLBACK}>
                <GrantedDraft />
              </LazyBoundary>
            }
          />
          <Route
            path="/search"
            element={
              <LazyBoundary fallback={SEARCH_FALLBACK}>
                <Search />
              </LazyBoundary>
            }
          />
          <Route
            path="/graph"
            element={
              <LazyBoundary fallback={GRAPH_FALLBACK}>
                <GraphView />
              </LazyBoundary>
            }
          />
          {/*
            Where an OAuth client's `?request=<id>` redirect lands. Eager,
            unlike the two editors and the reading screen above: it carries
            no editor weight of its own, and it is the one address a person
            arrives at mid-journey from somewhere else entirely, so it must
            not add a chunk fetch on top of that wait.
          */}
          <Route path="/authorize" element={<OauthConsent />} />
          {/*
            Offered to every role from the frame, and lazy all the same: most
            visits never open it, and the queue it draws is the largest screen
            the app shell used to carry.
          */}
          <Route
            path="/maintenance"
            element={
              <LazyBoundary fallback={MAINTENANCE_FALLBACK}>
                <Maintenance />
              </LazyBoundary>
            }
          />
          {/*
            The Administration area. Routed for everybody and rendered for
            admins only: the frame answers every address under it with the
            not-found screen for anyone else, so the area says exactly as
            much as a mistyped address does, and its pages never mount for
            them. `/admin` itself opens General. Maintenance is not in here:
            every role may open it, so it stays at `/maintenance` and the
            area only links to it.
          */}
          <Route
            path="/admin"
            element={
              <LazyBoundary fallback={ADMIN_FALLBACK}>
                <AdminArea />
              </LazyBoundary>
            }
          >
            <Route
              index
              element={<Navigate to={adminGeneralRoute()} replace />}
            />
            <Route
              path="general"
              element={
                <LazyBoundary fallback={ADMIN_FALLBACK}>
                  <AdminGeneral />
                </LazyBoundary>
              }
            />
            {/*
              One route for the list and for one account in it: the detail
              panel is part of the list screen, which keeps its search,
              selection and scroll while accounts open and close beside it.
            */}
            <Route
              path="users/:name?"
              element={
                <LazyBoundary fallback={USERS_FALLBACK}>
                  <UsersAdmin />
                </LazyBoundary>
              }
            />
            <Route
              path="agents"
              element={
                <LazyBoundary fallback={ADMIN_FALLBACK}>
                  <AdminAgents />
                </LazyBoundary>
              }
            />
            <Route
              path="invites"
              element={
                <LazyBoundary fallback={ADMIN_FALLBACK}>
                  <AdminInvites />
                </LazyBoundary>
              }
            />
            <Route
              path="github"
              element={
                <LazyBoundary fallback={SETTINGS_FALLBACK}>
                  <GithubSettings />
                </LazyBoundary>
              }
            />
            <Route path="*" element={<NotFound />} />
          </Route>
          {/*
            The two addresses admin pages had before the area: they move on
            with their query, or answer not-found for a non-admin.
          */}
          <Route path="/users" element={<MovedAdminPage to={usersRoute()} />} />
          <Route
            path="/settings/github"
            element={<MovedAdminPage to={githubSettingsRoute()} />}
          />
          {/*
            Routed for everybody and rendered for an account: an anonymous
            session has no profile, and the screen says so with the not-found
            screen the way the admin pages do.
          */}
          <Route
            path="/profile"
            element={
              <LazyBoundary fallback={PROFILE_FALLBACK}>
                <Profile />
              </LazyBoundary>
            }
          />
          <Route path="*" element={<NotFound />} />
        </Route>
      </Route>
    </Routes>
  );
}
