/**
 * Mounts an element the way the app shows an overlay from a scheduled
 * update, outside `act`, and sends keys the moment `selector` is in the
 * document: in the microtask after the commit, before the task React runs
 * passive effects in. `render` wraps its mount in `act`, which flushes
 * those effects at once and so hides the gap; this does not.
 */

import type { ReactElement } from "react";
import { createRoot } from "react-dom/client";

/** What `mountKeyingAtOnce` hands back: the keys sent, and the way out. */
export interface Keyed {
  /** The keydown events sent, in order, once `selector` appeared. */
  sent: KeyboardEvent[];
  /** Unmounts the element and removes its container. */
  unmount: () => void;
}

/** Resolves once the root's scheduled work and a task after it ran. */
function settled(ms = 50): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function mountKeyingAtOnce(
  element: ReactElement,
  selector: string,
  keys: KeyboardEventInit[],
): Promise<Keyed> {
  const flag = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean | undefined };
  const wasAct = flag.IS_REACT_ACT_ENVIRONMENT;
  flag.IS_REACT_ACT_ENVIRONMENT = false;
  const host = document.createElement("div");
  document.body.append(host);
  const sent: KeyboardEvent[] = [];
  const probe = new MutationObserver(() => {
    if (sent.length > 0 || !document.querySelector(selector)) return;
    probe.disconnect();
    for (const init of keys) {
      const event = new KeyboardEvent("keydown", {
        ...init,
        bubbles: true,
        cancelable: true,
      });
      sent.push(event);
      (document.activeElement ?? document.body).dispatchEvent(event);
    }
  });
  probe.observe(document.body, { childList: true, subtree: true });
  const root = createRoot(host);
  try {
    root.render(element);
    await settled();
  } finally {
    probe.disconnect();
    flag.IS_REACT_ACT_ENVIRONMENT = wasAct;
  }
  return {
    sent,
    unmount: () => {
      flag.IS_REACT_ACT_ENVIRONMENT = false;
      root.unmount();
      flag.IS_REACT_ACT_ENVIRONMENT = wasAct;
      host.remove();
    },
  };
}
