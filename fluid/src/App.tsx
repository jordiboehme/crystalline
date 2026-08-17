/**
 * The app, which is to say the order the providers go in.
 *
 * The order is the design. Query first, because the auth provider's probe is a
 * query. Theme next, so every screen including the ones the auth provider
 * renders instead of the app (the server-down banner, the disabled-account
 * screen) is drawn in the right theme. Tooltips above auth for the same reason
 * theme is: an icon button names itself through one, and the screens the auth
 * provider draws instead of the app have icon buttons in them too. Auth last,
 * so nothing below it renders before the app knows who is asking.
 *
 * The router is not here: it wraps this from `main.tsx`, which is what lets a
 * test mount the same tree on an in-memory history.
 */

import { CommandsProvider } from "./CommandsProvider";
import { AuthProvider } from "./auth/AuthProvider";
import { Tooltips } from "./components/primitives";
import { VersionSkewToast } from "./components/VersionSkewToast";
import { QueryProvider } from "./query/QueryProvider";
import { AppRoutes } from "./routes";
import { ThemeProvider } from "./theme/ThemeProvider";

export default function App() {
  return (
    <QueryProvider>
      <ThemeProvider>
        <Tooltips>
          <AuthProvider>
            <VersionSkewToast />
            {/*
              Inside the auth provider, because what a screen registers depends
              on what this session may do, and above the routes, because the
              palette that lists the actions outlives the screen that offered
              them.
            */}
            <CommandsProvider>
              <AppRoutes />
            </CommandsProvider>
          </AuthProvider>
        </Tooltips>
      </ThemeProvider>
    </QueryProvider>
  );
}
