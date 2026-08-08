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
import NotFound from "./screens/NotFound";
import Search from "./screens/Search";
import UsersAdmin from "./screens/UsersAdmin";

/**
 * The one lazy screen: everything CodeMirror rides in this chunk, so the app
 * shell never pays for the editor. The fallback matches the screen's own
 * skeleton so a prefetch miss is a quiet moment rather than a flash.
 */
const EngramEditor = lazy(() => import("./screens/EngramEditor"));

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
              <Suspense
                fallback={
                  <p className="text-sm text-slate-500 dark:text-slate-400">
                    Loading the editor
                  </p>
                }
              >
                <EngramEditor />
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
          <Route path="*" element={<NotFound />} />
        </Route>
      </Route>
    </Routes>
  );
}
