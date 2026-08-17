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

import { Suspense, lazy } from "react";
import { Route, Routes } from "react-router";

import LoginPage from "./auth/LoginPage";
import { RequireAuth } from "./auth/RequireAuth";
import { Layout } from "./components/Layout";
import { Skeleton } from "./components/Skeleton";
import DomainHome from "./screens/DomainHome";
import GraphView from "./screens/GraphView";
import Home from "./screens/Home";
import Maintenance from "./screens/Maintenance";
import ManifestPage from "./screens/ManifestPage";
import NotFound from "./screens/NotFound";
import Search from "./screens/Search";

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
const EngramEditor = lazy(() => import("./screens/EngramEditor"));
const ManifestEditor = lazy(() => import("./screens/ManifestEditor"));

/**
 * The reading screen, lazy for a third reason again.
 *
 * It is the screen this app exists to draw, so making it wait looks wrong
 * until the numbers are read: it pulls the markdown renderer's whole
 * neighborhood, the details and backlinks panels, the two dialogs and the
 * neighborhood section behind it, and every one of those bytes was being paid
 * for by the home screen, the search screen and the login screen too. The
 * primary deployment is the daemon serving this bundle off localhost, where a
 * second request for an already-built chunk is a round trip measured in
 * single-digit milliseconds - and the fallback is the exact skeleton the
 * screen shows itself while its own two requests are in flight, so a reader
 * sees one loading shape rather than a chunk fetch followed by a screen.
 */
const EngramPage = lazy(() => import("./screens/EngramPage"));

/**
 * And the two admin screens, which are lazy for a different reason: they weigh
 * almost nothing each, but they are screens only an admin ever opens, and the
 * app shell is paid for by everybody who opens the app at all. The accounts
 * table is the larger of the two and takes the most off the eager path.
 */
const GithubSettings = lazy(() => import("./screens/GithubSettings"));
const UsersAdmin = lazy(() => import("./screens/UsersAdmin"));

const EDITOR_FALLBACK = (
  <p className="text-sm text-slate-500 dark:text-slate-400">
    Loading the editor
  </p>
);

const SETTINGS_FALLBACK = (
  <p className="text-sm text-slate-500 dark:text-slate-400">Loading settings</p>
);

const USERS_FALLBACK = (
  <p className="text-sm text-slate-500 dark:text-slate-400">Loading accounts</p>
);

/**
 * The engram screen's own skeleton, spelled the same way it spells it: the
 * chunk arriving and the detail request landing are one wait to a reader, and
 * two different loading shapes in a row would say otherwise.
 */
const ENGRAM_FALLBACK = <Skeleton label="Loading the engram" rows={6} />;

export function AppRoutes() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route element={<RequireAuth />}>
        <Route element={<Layout />}>
          <Route index element={<Home />} />
          <Route path="/d/:domain" element={<DomainHome />} />
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
              <Suspense fallback={EDITOR_FALLBACK}>
                <EngramEditor />
              </Suspense>
            }
          />
          {/*
            Both MANIFEST routes sit above `/d/:domain/e/*` for readability,
            though nothing here collides: `manifest` is its own segment, not a
            permalink the splat below would otherwise swallow. The page is a
            static import - it renders markdown, no editor weight - while the
            editor rides its own lazy chunk, gated to admins by the screen
            itself the same way `/users` is.
          */}
          <Route path="/d/:domain/manifest" element={<ManifestPage />} />
          <Route
            path="/d/:domain/manifest/edit"
            element={
              <Suspense fallback={EDITOR_FALLBACK}>
                <ManifestEditor />
              </Suspense>
            }
          />
          <Route
            path="/d/:domain/e/*"
            element={
              <Suspense fallback={ENGRAM_FALLBACK}>
                <EngramPage />
              </Suspense>
            }
          />
          <Route path="/search" element={<Search />} />
          <Route path="/graph" element={<GraphView />} />
          {/*
            Eager, unlike the two admin screens below it: this one is offered
            to every role from the frame, so nobody would be spared its weight
            by making it wait, and it carries no editor and no graph engine.
          */}
          <Route path="/maintenance" element={<Maintenance />} />
          {/*
            Routed for everybody and rendered for admins only: the screen
            itself answers with the not-found screen for anyone else, so the
            address says exactly as much as a mistyped one does. Lazy for the
            same reason the settings screen is - nobody but an admin ever
            loads it - and the chunk carries the refusal too, which is what
            keeps the address from saying more than a mistyped one.
          */}
          <Route
            path="/users"
            element={
              <Suspense fallback={USERS_FALLBACK}>
                <UsersAdmin />
              </Suspense>
            }
          />
          {/*
            The same arrangement, one screen along: routed for everybody,
            rendered for admins only, and lazy because nobody else will ever
            load it.
          */}
          <Route
            path="/settings/github"
            element={
              <Suspense fallback={SETTINGS_FALLBACK}>
                <GithubSettings />
              </Suspense>
            }
          />
          <Route path="*" element={<NotFound />} />
        </Route>
      </Route>
    </Routes>
  );
}
