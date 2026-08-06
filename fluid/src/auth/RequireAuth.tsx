/**
 * The gate every screen except the login screen sits behind.
 *
 * There is exactly one condition: no account and no anonymous viewing. The
 * anonymous viewer is a real identity here, so it passes straight through; a
 * disabled account never reaches this point at all, because the provider
 * answered a 403 with its own screen.
 */

import { Navigate, Outlet, useLocation } from "react-router";

import { useAuth } from "./AuthContext";

/** Where the login screen looks for the place it should return you to. */
export interface FromLocation {
  from?: { pathname: string; search: string; hash: string };
}

export function RequireAuth() {
  const { user, capabilities } = useAuth();
  const location = useLocation();

  if (!user && !capabilities.anonymous) {
    // The intended location rides along, so signing in lands where the browser
    // was going rather than dumping everyone on the home screen.
    const state: FromLocation = {
      from: {
        pathname: location.pathname,
        search: location.search,
        hash: location.hash,
      },
    };
    return <Navigate to="/login" replace state={state} />;
  }

  return <Outlet />;
}
