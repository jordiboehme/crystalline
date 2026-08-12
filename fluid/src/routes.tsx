/**
 * Every address the app answers at.
 *
 * Declarative routing rather than a data router: the data layer is TanStack
 * Query, so the router has nothing to load and only has to match URLs. It is
 * also what lets the whole tree mount inside a `MemoryRouter` in a test with
 * no second route definition to keep in step. The builders that make links to
 * these patterns live in `paths.ts`, next door.
 *
 * `/evolve` is deliberately absent. It is reserved for the maintenance sweep
 * and unrouted in this slice, so it lands on the not-found screen rather than
 * existing as an empty promise; whoever adds it adds a screen with it.
 */

import { Suspense, lazy } from "react";
import { Route, Routes } from "react-router";

import LoginPage from "./auth/LoginPage";
import { RequireAuth } from "./auth/RequireAuth";
import { Layout } from "./components/Layout";
import DomainHome from "./screens/DomainHome";
import EngramPage from "./screens/EngramPage";
import GraphView from "./screens/GraphView";
import Home from "./screens/Home";
import ManifestPage from "./screens/ManifestPage";
import NotFound from "./screens/NotFound";
import Search from "./screens/Search";
import UsersAdmin from "./screens/UsersAdmin";

/**
 * The two lazy screens: everything CodeMirror rides in these chunks, so the
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
 * And the settings screen, which is lazy for a different reason: it weighs
 * almost nothing, but it is a screen only an admin ever opens, and the app
 * shell is paid for by everybody who opens the app at all.
 */
const GithubSettings = lazy(() => import("./screens/GithubSettings"));

const EDITOR_FALLBACK = (
  <p className="text-sm text-slate-500 dark:text-slate-400">
    Loading the editor
  </p>
);

const SETTINGS_FALLBACK = (
  <p className="text-sm text-slate-500 dark:text-slate-400">Loading settings</p>
);

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
          <Route path="/d/:domain/e/*" element={<EngramPage />} />
          <Route path="/search" element={<Search />} />
          <Route path="/graph" element={<GraphView />} />
          {/*
            Routed for everybody and rendered for admins only: the screen
            itself answers with the not-found screen for anyone else, so the
            address says exactly as much as a mistyped one does.
          */}
          <Route path="/users" element={<UsersAdmin />} />
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
