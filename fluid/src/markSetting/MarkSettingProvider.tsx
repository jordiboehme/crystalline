/**
 * Whether the editors show Markdown marks on the line being edited.
 *
 * Mirrors the theme provider: one value, remembered in local storage, with no
 * cross-tab sync (another open tab follows on reload).
 */

import { useCallback, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { MarkSettingContext } from "./context";
import type { MarkSettingValue } from "./context";

/** Where the setting is remembered: "on" when set, the key removed when off. */
export const MARK_SETTING_KEY = "fluid-marks-on-edit-line";

/** The stored setting, or off when there is none or storage is unreadable. */
function storedSetting(): boolean {
  try {
    return window.localStorage.getItem(MARK_SETTING_KEY) === "on";
  } catch {
    // Storage denied: off, and not remembered.
    return false;
  }
}

export function MarkSettingProvider({ children }: { children: ReactNode }) {
  const [showOnEditLine, setShow] = useState<boolean>(storedSetting);

  const setShowOnEditLine = useCallback((next: boolean) => {
    setShow(next);
    try {
      if (next) {
        window.localStorage.setItem(MARK_SETTING_KEY, "on");
      } else {
        window.localStorage.removeItem(MARK_SETTING_KEY);
      }
    } catch {
      // See storedSetting: storage is a convenience, never a requirement.
    }
  }, []);

  const value = useMemo<MarkSettingValue>(
    () => ({ showOnEditLine, setShowOnEditLine }),
    [showOnEditLine, setShowOnEditLine],
  );

  return <MarkSettingContext value={value}>{children}</MarkSettingContext>;
}
