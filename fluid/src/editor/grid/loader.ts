/**
 * The eager stub for the lazy grid chunk.
 *
 * Nothing here imports `./index` statically: the chunk is asked for with a
 * dynamic import, and only when a table the grid draws shows up in a view (or
 * a screen calls `prefetchGrid`). One compartment and one load for the whole
 * app - `previewConfig` runs again on every Raw flip, and a compartment made
 * per call would forget what the chunk brought. Until the chunk is in, and for
 * good if its import fails, nothing here changes the editor, so every table
 * keeps the read-only preview.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import { Compartment } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { ViewPlugin } from "@codemirror/view";

import { gridDraws } from "./draws";

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

/** One loader around one import; `tableGrid` below is the app's single one. */
export function tableGridWith(load: () => Promise<GridChunk>): TableGrid {
  const compartment = new Compartment();
  let loaded: Extension | null = null;
  let loading: Promise<Extension | null> | null = null;

  const request = (): Promise<Extension | null> => {
    loading ??= load().then(
      (chunk) => {
        loaded = chunk.gridExtension();
        return loaded;
      },
      () => null,
    );
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
        if (update.docChanged || update.viewportChanged) this.check();
      }

      check(): void {
        if (this.asked || !hasGridTable(this.view.state)) return;
        this.asked = true;
        void request().then((extension) => {
          if (extension === null || this.gone) return;
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
    extension: () => [compartment.of(loaded ?? []), watcher],
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
