/**
 * The marks setting and its hook, kept apart from the provider component so
 * each module exports one kind of thing and fast refresh stays reliable.
 */

import { createContext, use } from "react";

/** What the editors read: whether marks stay visible on the line being edited. */
export interface MarkSettingValue {
  /**
   * Whether the Markdown marks show on the line the caret is on. Off (the
   * default) keeps them hidden there too.
   */
  showOnEditLine: boolean;
  /** Choose the setting and remember it. */
  setShowOnEditLine: (next: boolean) => void;
}

export const MarkSettingContext = createContext<MarkSettingValue | null>(null);

/** The current setting. Throws outside a `MarkSettingProvider`, a wiring bug. */
export function useMarkSetting(): MarkSettingValue {
  const value = use(MarkSettingContext);
  if (!value) {
    throw new Error("useMarkSetting was called outside a MarkSettingProvider");
  }
  return value;
}
