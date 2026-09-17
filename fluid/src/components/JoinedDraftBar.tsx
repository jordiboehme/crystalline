/**
 * The bar that says you are typing in somebody else's draft.
 *
 * It sits at the top of the screen for as long as the join lasts, on every
 * screen, and that is the point of it: working inside another person's
 * unfolded work is a state a window is in rather than a thing one page does,
 * and a person who navigated away from the draft and then edited something
 * else has to be able to see, without looking for it, whose overlay their
 * writing is going into.
 *
 * The join lives in session storage (see `api/draftLinks`), so it ends when
 * the window does. This bar listens for the one event that module fires when
 * the stored join changes, because the storage event only fires in OTHER tabs
 * and the tab that joined is exactly the tab that has to redraw.
 */

import { useMutation } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useEffect, useState } from "react";

import type { HeldJoin } from "../api/draftLinks";
import {
  JOIN_CHANGED_EVENT,
  heldJoin,
  leaveDraft,
  rememberJoin,
} from "../api/draftLinks";

export function JoinedDraftBar(): ReactElement | null {
  const [join, setJoin] = useState<HeldJoin | null>(() => heldJoin());

  useEffect(() => {
    const refresh = (): void => {
      setJoin(heldJoin());
    };
    window.addEventListener(JOIN_CHANGED_EVENT, refresh);
    // The other tabs of the same browser never share this key - session
    // storage is per tab - so nothing else has to be listened for.
    return () => {
      window.removeEventListener(JOIN_CHANGED_EVENT, refresh);
    };
  }, []);

  const leaving = useMutation({
    mutationFn: async (held: HeldJoin) => {
      await leaveDraft(held.key);
    },
    // Forgotten whatever the server said. A join the server has already
    // dropped - the draft was folded, the daemon restarted - must not leave a
    // bar on the screen claiming this window is inside something.
    onSettled: () => {
      rememberJoin(null);
    },
  });

  if (!join) return null;

  return (
    <div
      role="status"
      className="flex flex-wrap items-center justify-between gap-2 border-b border-amber-300 bg-amber-50 px-4 py-2 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100"
    >
      <p>
        You are working in {join.owner}&apos;s draft of {join.path}
      </p>
      <button
        type="button"
        disabled={leaving.isPending}
        onClick={() => {
          leaving.mutate(join);
        }}
        className="rounded border border-amber-400 px-3 py-1 hover:bg-amber-100 disabled:opacity-50 dark:border-amber-600 dark:hover:bg-amber-900"
      >
        Leave
      </button>
    </div>
  );
}
