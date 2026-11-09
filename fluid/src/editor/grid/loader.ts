/**
 * The eager stub for the lazy grid chunk.
 *
 * Nothing here imports `./index` statically: the chunk is asked for with a
 * dynamic import, and only when a table the grid draws shows up in a view (or
 * a screen calls `prefetchGrid`). One compartment and one load for the whole
 * app - `previewConfig` runs again on every Raw flip, and a compartment made
 * per call would forget what the chunk brought. Until the chunk is in, nothing
 * here changes the editor, so every table keeps the read-only preview. A
 * failed import (a network blip, or a redeploy that changed the chunk's
 * hash) is tried again after 5 seconds, then after 30. After the third
 * failure the loader gives up: the compartment gets `gridUnavailable`, and
 * the table preview says to reload the page.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import { Compartment } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { ViewPlugin } from "@codemirror/view";

import { parseAdvanced } from "../parseProgress";
import { gridDraws, gridUnavailable } from "./draws";

interface GridChunk {
  gridExtension: () => Extension;
}

export interface TableGrid {
  /** The extension to place in a state: the compartment and its watcher. */
  extension: () => Extension;
  compartment: Compartment;
  /** Starts the load; resolves when it settled, whichever way. */
  prefetch: () => Promise<void>;
}

function hasGridTable(state: EditorState): boolean {
  let found = false;
  syntaxTree(state).iterate({
    enter: (node) => {
      if (found) return false;
      if (gridDraws(node, state.doc)) found = true;
      return !found;
    },
  });
  return found;
}

/** The waits before the second and the third try of a failed import. */
export const RETRY_DELAYS_MS = [5_000, 30_000];

/** What the compartment holds once the loader gave up. */
const unavailable: Extension = gridUnavailable.of(true);

/** One loader around one import; `tableGrid` below is the app's single one. */
export function tableGridWith(load: () => Promise<GridChunk>): TableGrid {
  const compartment = new Compartment();
  let loaded: Extension | null = null;
  let gaveUp = false;
  let failures = 0;
  let loading: Promise<Extension> | null = null;

  /** One try, and the next one after its wait; the give-up when none is left. */
  const attempt = (): Promise<Extension> =>
    load().then(
      (chunk) => {
        loaded = chunk.gridExtension();
        return loaded;
      },
      () => {
        const wait = RETRY_DELAYS_MS[failures];
        failures += 1;
        if (wait === undefined) {
          gaveUp = true;
          return unavailable;
        }
        return new Promise<void>((resolve) => {
          setTimeout(resolve, wait);
        }).then(attempt);
      },
    );

  /** Settles once: with the grid, or with `unavailable`. Never rejects. */
  const request = (): Promise<Extension> => {
    loading ??= attempt();
    return loading;
  };

  const watcher = ViewPlugin.fromClass(
    class {
      asked = false;
      gone = false;
      readonly view: EditorView;

      constructor(view: EditorView) {
        this.view = view;
        this.check();
      }

      update(update: ViewUpdate): void {
        // The first parse has a small time budget and can stop before the
        // table. The rest of the tree comes later in a transaction that
        // changes neither the text nor the viewport, so a tree that grew is
        // a reason to look again too.
        if (
          update.docChanged ||
          update.viewportChanged ||
          parseAdvanced(update)
        ) {
          this.check();
        }
      }

      check(): void {
        if (this.asked || !hasGridTable(this.view.state)) return;
        this.asked = true;
        void request().then((extension) => {
          if (this.gone) return;
          this.view.dispatch({ effects: compartment.reconfigure(extension) });
        });
      }

      destroy(): void {
        this.gone = true;
      }
    },
  );

  return {
    compartment,
    extension: () => [
      compartment.of(loaded ?? (gaveUp ? unavailable : [])),
      watcher,
    ],
    prefetch: async () => {
      await request();
    },
  };
}

const shared = tableGridWith(() => import("./index"));

/**
 * The table grid for a preview layer. Cheap to call again: every call hands
 * back the one compartment, holding the grid when it is already loaded.
 */
export function tableGrid(): Extension {
  return shared.extension();
}

/** Warms the chunk; screens call it on mount. Never rejects. */
export function prefetchGrid(): Promise<void> {
  return shared.prefetch();
}
