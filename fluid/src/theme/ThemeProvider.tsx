/**
 * Light or dark, and who decides.
 *
 * The choice is written to `data-theme` on the document element, which is what
 * the `dark` variant in index.css matches. Only `light` and `dark` are ever
 * written there: `system` is a preference, not a theme, and resolving it here
 * keeps every stylesheet reading one attribute instead of also consulting a
 * media query.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { ThemeContext } from "./context";
import type { ThemePreference, ThemeValue } from "./context";

/** Where the preference is remembered. Also read by the pre-paint script in index.html. */
const STORAGE_KEY = "fluid-theme";

/** The media query that answers what the operating system is set to. */
const DARK_QUERY = "(prefers-color-scheme: dark)";

/** The stored preference, or `system` when there is none or it is unreadable. */
function storedPreference(): ThemePreference {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === "light" || raw === "dark" || raw === "system") {
      return raw;
    }
  } catch {
    // A browser with storage denied still gets a working app; it just does not
    // remember the choice between visits.
  }
  return "system";
}

/** Whether the system is asking for dark. False where `matchMedia` is missing. */
function systemPrefersDark(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia(DARK_QUERY).matches
  );
}

/**
 * Hold the theme preference, resolve it and keep `data-theme` in step.
 */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [preference, setPreference] =
    useState<ThemePreference>(storedPreference);
  const [systemDark, setSystemDark] = useState(systemPrefersDark);

  // Following the system means following it while the app is open, not only at
  // startup: someone flipping their laptop to dark at sunset should see it.
  useEffect(() => {
    if (typeof window.matchMedia !== "function") {
      return;
    }
    const query = window.matchMedia(DARK_QUERY);
    const onChange = (event: MediaQueryListEvent) => {
      setSystemDark(event.matches);
    };
    query.addEventListener("change", onChange);
    return () => {
      query.removeEventListener("change", onChange);
    };
  }, []);

  const resolved: ThemeValue["resolved"] =
    preference === "system" ? (systemDark ? "dark" : "light") : preference;

  useEffect(() => {
    document.documentElement.dataset.theme = resolved;
  }, [resolved]);

  const choose = useCallback((next: ThemePreference) => {
    setPreference(next);
    try {
      window.localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // See storedPreference: storage is a convenience, never a requirement.
    }
  }, []);

  const value = useMemo<ThemeValue>(
    () => ({ preference, resolved, choose }),
    [preference, resolved, choose],
  );

  return <ThemeContext value={value}>{children}</ThemeContext>;
}
