/**
 * The theme context and its hook, kept apart from the provider component so
 * each module exports one kind of thing and fast refresh stays reliable.
 */

import { createContext, use } from "react";

/** What someone can ask for: a fixed theme, or whatever the system says. */
export type ThemePreference = "system" | "light" | "dark";

/** What the app reads: the preference, what it resolves to, and how to change it. */
export interface ThemeValue {
  /** What was asked for. */
  preference: ThemePreference;
  /** What that means right now, and what `data-theme` is set to. */
  resolved: "light" | "dark";
  /** Choose a preference and remember it. */
  choose: (preference: ThemePreference) => void;
}

export const ThemeContext = createContext<ThemeValue | null>(null);

/** The current theme. Throws outside a `ThemeProvider`, which is a wiring bug. */
export function useTheme(): ThemeValue {
  const value = use(ThemeContext);
  if (!value) {
    throw new Error("useTheme was called outside a ThemeProvider");
  }
  return value;
}
